use super::*;

const A: &str = "avtr_11111111-2222-4333-8444-555555555555";
const W: &str = "wrld_11111111-2222-4333-8444-555555555555";

fn of(bytes: &[u8]) -> (Vec<String>, bool) {
    let (ids, whole) = ids_in(bytes, bytes.len() as u64).unwrap();
    (ids.into_iter().collect(), whole)
}

/// A document with `body` in it, and what Unity writes after.
fn yaml(body: &str) -> (Vec<String>, bool) {
    let object = if body.starts_with("MonoBehaviour:") || body.starts_with("PrefabInstance:") { "" } else { "MonoBehaviour:\n" };
    let end = if body.ends_with('\n') { "" } else { "\n" };
    of(format!("%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!114 &1\n{object}{body}{end}  m_End: 0\n").as_bytes())
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
    let marked = [&b"\xef\xbb\xbf"[..], format!("%YAML 1.1\n--- !u!114 &1\nMonoBehaviour:\n  blueprintId: {A}\n").as_bytes()].concat();
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
        let after = if version >= 22 { 48 } else { 20 };
        let mut out = vec![0u8; after];
        out[8..12].copy_from_slice(&version.to_be_bytes());
        out.extend_from_slice(b"2022.3.22f1\0");
        let data = out.len() as u32;
        out.extend_from_slice(body);
        let len = out.len();
        if version >= 22 {
            out[20..24].copy_from_slice(&12u32.to_be_bytes());
            out[24..32].copy_from_slice(&(len as u64).to_be_bytes());
            out[32..40].copy_from_slice(&u64::from(data).to_be_bytes());
        } else {
            out[0..4].copy_from_slice(&12u32.to_be_bytes());
            out[4..8].copy_from_slice(&(len as u32).to_be_bytes());
            out[12..16].copy_from_slice(&data.to_be_bytes());
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
    // The right version and length are not enough: something else with those numbers.
    for version in [17u32, 22] {
        let mut other = vec![7u8; 1000];
        other[8..12].copy_from_slice(&version.to_be_bytes());
        other[4..8].copy_from_slice(&1000u32.to_be_bytes());
        other[24..32].copy_from_slice(&1000u64.to_be_bytes());
        assert!(!of(&other).1, "{version}");
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

/// VRChat's own samples are nobody's upload, by their very IDs; any other ID counts wherever it is, in a package named like
/// VRChat's too.
#[test]
fn only_the_sdks_own_sample_ids_are_left_out() {
    let mut c = Content::default();
    let mut read = |at: &str, id: &str| {
        let text = crate::unity6::trial::tests::scene(id);
        c.read(Path::new(at), text.as_bytes(), text.len() as u64).unwrap();
    };
    read("Packages/com.vrchat.worlds/Samples/x.unity", OF_THE_SDK[0]);
    read("Assets/Samples/x.unity", OF_THE_SDK[1]);
    read("Packages/com.vrchat.worlds/Samples/mine.unity", W);
    read("Packages/com.other/x.prefab", A);
    assert_eq!(c.ids.iter().collect::<Vec<_>>(), [A, W]);
    assert!(c.unsure.is_empty());
    assert!(carries(Path::new("a.UNITY")) && carries(Path::new("a.prefab")) && carries(Path::new("a.asset")) && !carries(Path::new("a.mat")));
}

/// Fourth verification, Major 7: a text that does not have Unity's shape to its end may be missing what it had.
#[test]
fn a_text_cut_short_or_not_unitys_may_carry_one() {
    let whole = |text: &str| of(text.as_bytes()).1;
    let head = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";
    let good = format!("{head}--- !u!1 &10\nGameObject:\n  m_Component:\n  - component: {{fileID: 2}}\n  m_Name: 'it''s {{a}}'\n  m_Text: \"two\\\"\n    lines [\\\n    of it\"\n  m_Plain: a [b {{c\n  m_Long: {{fileID: 11500000, guid: 0123456789abcdef0123456789abcdef,\n    type: 3}}\n  m_Empty: \n  m_List: []\n--- !u!4 &20 stripped\nTransform:\n  m_Children:\n  - {{fileID: 1}}\n");
    assert!(whole(&good));
    assert!(whole(&good.replace('\n', "\r\n")));
    // Cut anywhere: between documents it cannot be told from a whole one (an object it had is not known of); everywhere else it is.
    let ends: Vec<usize> = good.match_indices('\n').map(|(i, _)| i + 1).collect();
    for cut in 0..good.len() {
        if !ends.contains(&cut) {
            assert!(!whole(&good[..cut]), "{:?}", &good[..cut]);
        }
    }
    for bad in [
        "", head, &format!("{head}--- !u!1 &10\n"), &format!("{head}--- !u!1 &10\nGameObject:\n"), &format!("{head}--- !u!1 &10\nGameObject:\n  m_List:\n"), &format!("{head}--- !u!1 &10\nGameObject:\n  m_List: [1, 2\n"),
        &format!("{head}--- !u!1 &10\nGameObject:\n  m_Mods:\n  - target: {{fileID: 1\n"), &format!("{head}--- !u!1 &10\nGameObject:\n  m_T: \"no end\n  a: 1\n"), &format!("{head}GameObject:\n  a: 1\n"), &format!("{head}--- other\nGameObject:\n  a: 1\n"),
        &format!("{head}--- !u!1 &10\nGameObject:\n  a: 1\n<<<<<<< HEAD\n  b: 1\n=======\n  b: 2\n>>>>>>> x\n  c: 1\n"), &format!("{head}--- !u!1 &10\nGameObject:\n\ta: 1\n  b: 1\n"), &format!("{head}--- !u!1 &10\n--- !u!1 &11\nGameObject:\n  a: 1\n"),
        &format!("{head}--- !u!1 &10\nGameObject:\n  a: }}\n"), &format!("{head}--- !u!1 &10\nGameObject:\n  a: 1\n  b: 2"),
    ] {
        assert!(!whole(bad), "{bad:?}");
    }
    assert!(!of(b"%YAML 1.1\n--- !u!1 &10\nGameObject:\n  m_Name: \xff\xfe\n  a: 1\n").1, "not text");
}

/// What an archive holds is read as it comes; what cannot be read whole may hold one.
#[test]
fn archives_are_read_whole_or_may_carry_one() {
    use crate::unity6::trial::tests::{archive, scene};
    let temp = tempfile::tempdir().unwrap();
    let at = temp.path().join("a.tgz");
    let read = |bytes: &[u8]| {
        let mut c = Content::default();
        c.read_archive(Path::new("Packages/a.tgz"), bytes).unwrap();
        c
    };
    let long = format!("package/{}/B.PREFAB", "x".repeat(150));
    archive(&at, &[("package/package.json", "{}".into()), (&long, scene(A)), ("package/readme.md", W.into()), (&long.replace("B.PREFAB", "C.unity"), scene(W)), ("package/D.asset", "not Unity's".into())]);
    let whole = std::fs::read(&at).unwrap();
    let c = read(&whole);
    assert_eq!(c.ids.iter().collect::<Vec<_>>(), [A, W]);
    assert_eq!(c.unsure.len(), 1, "{:?}", c.unsure);
    assert!(c.unsure[0].contains("D.asset"));
    // None at all in one that is whole.
    archive(&at, &[("package/package.json", "{}".into())]);
    assert_eq!(read(&std::fs::read(&at).unwrap()), Content::default());
    // Cut short, altered, not one.
    for bad in [&whole[..whole.len() - 1], &whole[..20], b"", b"PK\x03\x04"] {
        assert!(read(bad).unsure.iter().any(|u| u.ends_with("a.tgz")), "{} bytes", bad.len());
    }
    // A link, and a header that does not add up.
    let mut tar = vec![];
    flate2::read::GzDecoder::new(&whole[..]).read_to_end(&mut tar).unwrap();
    let pack = |tar: &[u8]| {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::fast());
        gz.write_all(tar).unwrap();
        gz.finish().unwrap()
    };
    let mut odd = tar.clone();
    odd[156] = b'2';
    assert!(!read(&pack(&odd)).unsure.is_empty());
    let mut odd = tar.clone();
    odd[3] ^= 1;
    assert!(!read(&pack(&odd)).unsure.is_empty());
    let mut long = tar.clone();
    long.truncate(512);
    assert!(!read(&pack(&long)).unsure.is_empty());
    // An error of the disk is an error, not "may carry".
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the disk"))
        }
    }
    assert!(Content::default().read_archive(Path::new("a.tgz"), Broken).is_err());
}
