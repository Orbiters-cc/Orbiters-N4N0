//! The content IDs (`avtr_…`, `wrld_…`, `prop_…`) a project's files say they were uploaded as, read from every file that can
//! carry VRChat's Pipeline Manager: each scene, prefab and asset of `Assets` and of every package that is part of the project
//! (embedded, local, kept only in its Library). A file that cannot be read **as Unity's** may carry one: it is
//! [`Content::unsure`], never "none".

use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

/// What a project's files were uploaded as.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Content {
    pub ids: BTreeSet<String>,
    /// Files that may carry an ID Orbiters cannot read.
    pub unsure: Vec<String>,
}

/// Samples and tests in VRChat's own packages (every SDK release from 3.4.0 to 3.10.6, betas included) come with an ID of VRChat's: every project with that package has it,
/// and nobody's upload is told by it.
const OF_THE_SDK: [&str; 7] = [
    "avtr_c38a1615-5bf5-42b4-84eb-a8b6c37cbd11",
    "wrld_47818e3e-af8b-4238-a705-74a9c39c211d",
    "wrld_5f06685d-baaf-40fc-9055-5fadf53eea8f",
    "wrld_6e0cc7b3-51c3-4ecb-9c3e-1d178c6c2393",
    "wrld_924bd567-0855-4499-95e4-4f6358db8317",
    "wrld_9f5e0df7-0839-459b-8cdf-93b60a656686",
    "wrld_d5953f6d-5dae-4fe3-b662-128ef2b72267",
];

const ID: usize = 5 + 36;
/// A line is looked at whole up to this; what is past it is only looked through for the field's name.
const LINE: usize = 64 * 1024;

/// Whether the file can carry a Pipeline Manager, by its kind.
pub fn carries(at: &Path) -> bool {
    ["unity", "prefab", "asset"].iter().any(|x| at.extension().is_some_and(|e| e.eq_ignore_ascii_case(x)))
}

/// Whether `at` is in one of VRChat's own packages of the project.
fn sdk(at: &Path) -> bool {
    let mut names = at.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    names.next().as_deref() == Some("packages") && names.next().is_some_and(|n| n.starts_with("com.vrchat."))
}

impl Content {
    /// Takes in the file `at` of the project, `len` bytes long.
    pub fn read(&mut self, at: &Path, file: impl Read, len: u64) -> io::Result<()> {
        let (ids, whole) = ids_in(file, len)?;
        let ids = ids.into_iter().filter(|id| !OF_THE_SDK.contains(&id.as_str()));
        if sdk(at) {
            // A sample of VRChat's in an SDK not known here, or somebody's own upload of one: it may be either.
            self.unsure.extend(ids.map(|id| format!("{} ({id})", at.display())));
        } else {
            self.ids.extend(ids);
        }
        if !whole {
            self.unsure.push(at.display().to_string());
        }
        Ok(())
    }

    /// Something that may carry an ID and could not be looked at.
    pub fn unreadable(&mut self, what: String) {
        self.unsure.push(what);
    }

    pub fn add(&mut self, other: Content) {
        self.ids.extend(other.ids);
        self.unsure.extend(other.unsure.into_iter().filter(|u| !self.unsure.contains(u)).collect::<Vec<_>>());
    }
}

/// The IDs written anywhere in `text`.
fn find(text: &[u8], out: &mut BTreeSet<String>) {
    for prefix in [&b"avtr_"[..], b"wrld_", b"prop_"] {
        let mut rest = text;
        while let Some(i) = rest.windows(prefix.len()).position(|w| w == prefix) {
            let id = &rest[i..rest.len().min(i + ID)];
            let uuid = &id[prefix.len()..];
            if uuid.len() == 36 && uuid.iter().all(|b| b.is_ascii_hexdigit() || *b == b'-') {
                out.insert(String::from_utf8_lossy(id).to_lowercase());
            }
            rest = &rest[i + prefix.len()..];
        }
    }
}

fn says(line: &[u8], word: &[u8]) -> bool {
    line.windows(word.len()).any(|w| w == word)
}

/// The IDs of one file, and whether all of it was read as Unity writes one.
pub fn ids_in(mut file: impl Read, len: u64) -> io::Result<(BTreeSet<String>, bool)> {
    let mut head = vec![];
    (&mut file).take(64).read_to_end(&mut head)?;
    if head.starts_with(b"%YAML") || head.starts_with(b"\xef\xbb\xbf%YAML") {
        return text(head.as_slice().chain(file));
    }
    let known = serialized(&head, len);
    // Not compressed and not encoded: an ID in it is there as it is. In a file that is not Unity's, what looks like one counts
    // too (a warning too many costs less than one too few).
    let (mut ids, mut buffer, mut kept) = (BTreeSet::new(), vec![0u8; 256 * 1024], head);
    loop {
        find(&kept, &mut ids);
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok((ids, known));
        }
        kept.drain(..kept.len().saturating_sub(ID - 1));
        kept.extend_from_slice(&buffer[..n]);
    }
}

/// Whether `head` begins one of Unity's binary files of `len` bytes.
fn serialized(head: &[u8], len: u64) -> bool {
    let u32_at = |i: usize| head.get(i..i + 4).map(|b| u64::from(u32::from_be_bytes(b.try_into().unwrap())));
    match u32_at(8) {
        Some(9..=21) => u32_at(4) == Some(len),
        Some(22..=40) => head.get(24..32).map(|b| u64::from_be_bytes(b.try_into().unwrap())) == Some(len),
        _ => false,
    }
}

/// One line into `line` (its end left out), as far as [`LINE`]; whether there was one, and whether what is past that has the
/// field's name in it.
fn next_line(r: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<(bool, bool)> {
    const FIELD: &[u8] = b"blueprintId";
    line.clear();
    let (mut any, mut beyond, mut tail) = (false, false, Vec::<u8>::new());
    loop {
        let piece = r.fill_buf()?;
        if piece.is_empty() {
            return Ok((any, beyond));
        }
        any = true;
        let end = piece.iter().position(|b| *b == b'\n');
        let part = &piece[..end.unwrap_or(piece.len())];
        let room = LINE - line.len();
        line.extend_from_slice(&part[..part.len().min(room)]);
        if part.len() > room {
            tail.drain(..tail.len().saturating_sub(FIELD.len()));
            tail.extend_from_slice(&part[room..]);
            beyond |= says(&tail, FIELD);
        }
        let used = end.map_or(piece.len(), |e| e + 1);
        r.consume(used);
        if end.is_some() {
            return Ok((true, beyond));
        }
    }
}

/// Unity's text: an ID counts where a blueprint ID is (a portal names other worlds), which is a component's own field or a
/// prefab override's value under its property's name. Such a place with something that is not an ID may still be one.
fn text(file: impl Read) -> io::Result<(BTreeSet<String>, bool)> {
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let (mut ids, mut whole, mut property, mut line) = (BTreeSet::new(), true, false, vec![]);
    let after = |line: &[u8], word: &[u8]| -> Option<Vec<u8>> {
        let at = line.windows(word.len()).position(|w| w == word)? + word.len();
        Some(line[at..].to_vec())
    };
    loop {
        let (any, beyond) = next_line(&mut r, &mut line)?;
        if !any {
            return Ok((ids, whole));
        }
        // Past the part that was looked at, or not text at all.
        whole &= !beyond && !line.contains(&0);
        if says(&line, b"propertyPath:") {
            property = says(&line, b"blueprintId");
        }
        let value = after(&line, b"blueprintId:").or_else(|| property.then(|| after(&line, b"value:")).flatten());
        let Some(value) = value else { continue };
        let mut here = BTreeSet::new();
        find(&value, &mut here);
        let blank = value.iter().all(|b| b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b'}' | b','));
        whole &= !here.is_empty() || blank;
        ids.extend(here);
    }
}

#[cfg(test)]
mod tests;
