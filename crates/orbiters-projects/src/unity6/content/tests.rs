use super::*;

const A: &str = "avtr_11111111-2222-4333-8444-555555555555";
const W: &str = "wrld_11111111-2222-4333-8444-555555555555";

fn of(bytes: &[u8]) -> (Vec<String>, bool) {
    let (ids, whole) = ids_in(bytes, bytes.len() as u64).unwrap();
    (ids.into_iter().collect(), whole)
}

fn yaml(body: &str) -> (Vec<String>, bool) {
    of(format!("%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!114 &1\n{body}").as_bytes())
}

#[test]
fn ids_are_found_where_unity_keeps_them() {
    assert_eq!(yaml(&format!("MonoBehaviour:\n  blueprintId: {A}\n")), (vec![A.into()], true));
    assert_eq!(yaml(&format!("MonoBehaviour:\r\n  blueprintId: {}\r\n", A.to_uppercase().replace("AVTR", "avtr"))), (vec![A.into()], true));
    // An override in a variant, and in a variant of a variant.
    let over = |id: &str| format!("PrefabInstance:\n    - target: {{fileID: 1}}\n      propertyPath: blueprintId\n      value: {id}\n      objectReference: {{fileID: 0}}\n");
    assert_eq!(yaml(&format!("{}{}", over(A), over(W))), (vec![A.into(), W.into()], true));
    // On one line, quoted.
    assert_eq!(yaml(&format!("    - {{propertyPath: blueprintId, value: '{A}'}}\n")), (vec![A.into()], true));
    // A portal to another world, a name, an unuploaded avatar, an override of something else.
    assert_eq!(yaml(&format!("  roomId: {W}\n  blueprintId: \n  blueprintId: ''\n      propertyPath: roomId\n      value: {W}\n      propertyPath: blueprintId\n      value: \n")), (vec![], true));
    // The mark a text editor puts first.
    let marked = [&b"\xef\xbb\xbf"[..], format!("%YAML 1.1\n  blueprintId: {A}").as_bytes()].concat();
    assert_eq!(of(&marked), (vec![A.into()], true));
}

/// What may carry an ID and cannot be read is never "none".
#[test]
fn what_cannot_be_read_may_carry_one() {
    // The field with something that is not an ID (an old avatar's, a tool's placeholder).
    for value in ["8JoV9XEdpo", "avtr_x", "{{ID}}"] {
        assert_eq!(yaml(&format!("  blueprintId: {value}\n")), (vec![], false), "{value}");
        assert_eq!(yaml(&format!("      propertyPath: blueprintId\n      value: {value}\n")), (vec![], false), "{value}");
    }
    // Not Unity's: empty, text, a stub of a large-file store, half a file, compressed, encrypted.
    for bytes in [&b""[..], b"hello", b"version https://git-lfs.github.com/spec/v1\noid sha256:aa\n", b"YAML 1.1\n", b"UnityFS\0\0\0", &[0xff; 4096]] {
        assert_eq!(of(bytes), (vec![], false), "{bytes:?}");
    }
    // Text with bytes that are not.
    assert!(!of(b"%YAML 1.1\n\0\0\0\n").1);
    // One in a file that is not Unity's counts, and the file may carry others.
    assert_eq!(of(format!("\0\0UnityFS\0{W}\0").as_bytes()), (vec![W.into()], false));
}

/// Unity's binary files: whole ones hold their texts as they are, so one without an ID has none.
#[test]
fn a_binary_file_is_read_whole() {
    let file = |version: u32, body: &[u8]| {
        let mut out = vec![0u8; 48];
        out[8..12].copy_from_slice(&version.to_be_bytes());
        out.extend_from_slice(body);
        let len = out.len();
        if version >= 22 {
            out[24..32].copy_from_slice(&(len as u64).to_be_bytes());
        } else {
            out[4..8].copy_from_slice(&(len as u32).to_be_bytes());
        }
        out
    };
    for version in [17, 21, 22] {
        assert_eq!(of(&file(version, &[7; 1000])), (vec![], true), "{version}");
        // Anywhere, across the pieces it is read in.
        for at in [0, 10, 256 * 1024 - 30, 256 * 1024 + 48 - 20, 600_000] {
            let mut body = vec![7; at];
            body.extend_from_slice(A.as_bytes());
            body.extend([0; 100]);
            assert_eq!(of(&file(version, &body)), (vec![A.into()], true), "{version} at {at}");
        }
        let mut cut = file(version, &[7; 1000]);
        cut.pop();
        assert!(!of(&cut).1, "cut short");
    }
}

/// A line as long as a mesh: read in bounded memory, and nothing in it is missed silently.
#[test]
fn a_line_without_an_end_is_not_a_way_around() {
    let long = "0".repeat(300_000);
    assert_eq!(yaml(&format!("  _typelessdata: {long}\n  blueprintId: {A}\n")), (vec![A.into()], true));
    assert_eq!(yaml(&format!("  a: {long} blueprintId: {A}\n")).1, false);
    for at in LINE - 12..LINE + 2 {
        let body = format!("{} blueprintId: {A}", "0".repeat(at));
        let (ids, whole) = yaml(&body);
        assert!(!ids.is_empty() || !whole, "{at}");
    }
    // The value alone past it, and an override's.
    for at in LINE - 60..LINE + 2 {
        let body = format!("{}blueprintId:{}{A}", "0".repeat(LINE - 12), " ".repeat(at - (LINE - 60)));
        assert!(!yaml(&body).1, "{at}");
        let body = format!("  propertyPath: blueprintId\n{}value: {A}", "0".repeat(at));
        let (ids, whole) = yaml(&body);
        assert!(!ids.is_empty() || !whole, "{at}");
    }
    assert!(!carries(Path::new("ProjectSettings/ProjectSettings.asset")));
}

/// VRChat's own samples are nobody's upload; anything else in its packages may be.
#[test]
fn the_sdks_own_samples_are_told_apart() {
    let mut c = Content::default();
    let file = |id: &str| format!("%YAML 1.1\n  blueprintId: {id}\n");
    let mut read = |at: &str, id: &str| {
        let text = file(id);
        c.read(Path::new(at), text.as_bytes(), text.len() as u64).unwrap();
    };
    read("Packages/com.vrchat.worlds/Samples/x.unity", OF_THE_SDK[0]);
    read("Assets/Samples/x.unity", OF_THE_SDK[1]);
    read("Packages/com.vrchat.worlds/Samples/mine.unity", W);
    read("Packages/com.other/x.prefab", A);
    assert_eq!(c.ids.iter().collect::<Vec<_>>(), [A]);
    assert_eq!(c.unsure.len(), 1);
    assert!(c.unsure[0].contains(W));
    assert!(carries(Path::new("a.UNITY")) && carries(Path::new("a.prefab")) && carries(Path::new("a.asset")) && !carries(Path::new("a.mat")));
}
