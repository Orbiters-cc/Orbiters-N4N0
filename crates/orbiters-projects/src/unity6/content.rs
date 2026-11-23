//! The content IDs (`avtr_…`, `wrld_…`, `prop_…`) a project's files say they were uploaded as, read from every file that can
//! carry VRChat's Pipeline Manager: each scene, prefab and asset of `Assets` and of every package that is part of the project
//! (embedded, local folders, **local archives**, kept only in its Library), whatever the package is called. A file that cannot
//! be read whole **as Unity's** (or an archive that cannot be read whole) may carry one: it is [`Content::unsure`], never "none".

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

/// Samples and tests in VRChat's own packages (every SDK release from 3.4.0 to 3.10.6, betas included) come with an ID of
/// VRChat's: every project with that package has it, and nobody's upload is told by it. Only these values: any other ID counts,
/// in a package named like VRChat's too.
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
const FIELD: &[u8] = b"blueprintId";

/// Whether the file can carry a Pipeline Manager, by its kind (a project's settings are no objects of a scene).
pub fn carries(at: &Path) -> bool {
    let settings = at.components().next().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case("ProjectSettings"));
    !settings && ["unity", "prefab", "asset"].iter().any(|x| at.extension().is_some_and(|e| e.eq_ignore_ascii_case(x)))
}

impl Content {
    /// Takes in the file `at` of the project, `len` bytes long.
    pub fn read(&mut self, at: &Path, file: impl Read, len: u64) -> io::Result<()> {
        let (ids, whole) = ids_in(file, len)?;
        self.ids.extend(ids.into_iter().filter(|id| !OF_THE_SDK.contains(&id.as_str())));
        if !whole {
            self.unsure.push(at.display().to_string());
        }
        Ok(())
    }

    /// Takes in what is in the package archive `at` of the project (a `.tgz`, read as it comes: nothing is put on a disk).
    pub fn read_archive(&mut self, at: &Path, file: impl Read) -> io::Result<()> {
        let mut inside = Content::default();
        let whole = archive(file, &mut |name, mut member, len| {
            let shown = at.join(name);
            // (A member's path is only ever compared and shown.)
            if carries(Path::new(name)) {
                inside.read(&shown, &mut member, len)?;
            }
            Ok(())
        })?;
        self.add(inside);
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

/// The most an archive is read for.
const MOST: u64 = 8 * 1024 * 1024 * 1024;

/// Goes through a `.tgz`: each file in it by its name. Whether all of it was read: not a gzip, cut short, a member of a kind that
/// is not known here, a name that is not text, or more than [`MOST`], and it was not.
fn archive(file: impl Read, each: &mut dyn FnMut(&str, &mut dyn Read, u64) -> io::Result<()>) -> io::Result<bool> {
    // An error of the disk is one; an error of the archive is what it is like.
    let mut disk = Disk { file, failed: None };
    let whole = members(&mut flate2::read::MultiGzDecoder::new(&mut disk).take(MOST), each);
    match disk.failed {
        Some(e) => Err(e),
        None => Ok(whole.unwrap_or(false)),
    }
}

struct Disk<R> {
    file: R,
    failed: Option<io::Error>,
}

impl<R: Read> Read for Disk<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer).inspect_err(|e| self.failed = Some(io::Error::new(e.kind(), e.to_string())))
    }
}

fn members(tar: &mut impl Read, each: &mut dyn FnMut(&str, &mut dyn Read, u64) -> io::Result<()>) -> io::Result<bool> {
    let text = |b: &[u8]| std::str::from_utf8(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).map(|t| t.trim_end().to_string());
    let (mut named, mut whole, mut ends) = (None::<String>, true, 0);
    loop {
        let mut head = [0u8; 512];
        tar.read_exact(&mut head)?;
        if head.iter().all(|b| *b == 0) {
            // Two empty blocks end it.
            ends += 1;
            if ends == 2 {
                return Ok(whole);
            }
            continue;
        }
        ends = 0;
        // Its own checksum, the field itself counted as spaces (writers that leave it out are not npm's or Unity's).
        let sum: u32 = head.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { u32::from(*b) }).sum();
        let says = text(&head[148..156]).ok().and_then(|t| u32::from_str_radix(t.trim(), 8).ok());
        whole &= says.is_none_or(|s| s == sum);
        let size = if head[124] & 0x80 != 0 { head[128..136].iter().fold(0u64, |n, b| n << 8 | u64::from(*b)) } else { u64::from_str_radix(text(&head[124..136]).unwrap_or_default().trim(), 8).map_err(io::Error::other)? };
        let body = size.div_ceil(512) * 512;
        let kind = head[156];
        let mut short = |most: u64| -> io::Result<Option<String>> {
            if size > most {
                return Ok(None);
            }
            let mut bytes = vec![];
            (&mut *tar).take(body).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != body {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            bytes.truncate(size as usize);
            Ok(String::from_utf8(bytes).ok())
        };
        match kind {
            // A longer name for the next one, in GNU's way and in POSIX's.
            b'L' => match short(64 * 1024)? {
                Some(name) => named = Some(name.trim_end_matches('\0').to_string()),
                None => return Ok(false),
            },
            b'x' => match short(1024 * 1024)? {
                Some(records) => {
                    // `<length> path=<name>\n`, one record a line.
                    if let Some(path) = records.lines().find_map(|l| l.split_once(' ')?.1.strip_prefix("path=")) {
                        named = Some(path.to_string());
                    }
                    // The size of the next one is said there too when it is too large for its own header.
                    whole &= !records.lines().any(|l| l.split_once(' ').is_some_and(|(_, r)| r.starts_with("size=")));
                }
                None => return Ok(false),
            },
            b'g' => {
                if short(1024 * 1024)?.is_none() {
                    return Ok(false);
                }
            }
            0 | b'0' | b'7' => {
                let name = match named.take() {
                    Some(name) => Some(name),
                    None => {
                        let prefix = if head[257..262] == *b"ustar" { text(&head[345..500]).unwrap_or_default() } else { String::new() };
                        text(&head[..100]).ok().map(|n| if prefix.is_empty() { n } else { format!("{prefix}/{n}") })
                    }
                };
                let mut member = (&mut *tar).take(body);
                match name {
                    Some(name) => each(&name, &mut (&mut member).take(size), size)?,
                    None => whole = false,
                }
                let left = io::copy(&mut member, &mut io::sink())?;
                let _ = left;
                if member.limit() != 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
            }
            // A folder.
            b'5' => named = None,
            // A link to another member or elsewhere, or something else.
            _ => {
                named = None;
                whole = false;
                io::copy(&mut (&mut *tar).take(body), &mut io::sink())?;
            }
        }
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
    (&mut file).take(128).read_to_end(&mut head)?;
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

/// Whether `head` begins one of Unity's binary files of `len` bytes: its sizes agree with each other and with the file's, and the
/// version of Unity that wrote it follows them.
fn serialized(head: &[u8], len: u64) -> bool {
    let u32_at = |i: usize| head.get(i..i + 4).map(|b| u64::from(u32::from_be_bytes(b.try_into().unwrap())));
    let u64_at = |i: usize| head.get(i..i + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap()));
    let (metadata, size, data, after) = match u32_at(8) {
        Some(9..=21) => (u32_at(0), u32_at(4), u32_at(12), 20),
        Some(22..=40) => (u32_at(20), u64_at(24), u64_at(32), 48),
        _ => return false,
    };
    let (Some(metadata), Some(size), Some(data)) = (metadata, size, data) else { return false };
    let little_or_big = head.get(16).is_some_and(|b| *b <= 1);
    let version = head.get(after..).map(|b| &b[..b.iter().position(|c| *c == 0).unwrap_or(0)]).unwrap_or_default();
    let written_by = version.len() >= 5 && version[0].is_ascii_digit() && version.contains(&b'.') && version.iter().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(b));
    size == len && little_or_big && written_by && metadata > 0 && data <= len && metadata.saturating_add(after as u64) <= data.max(metadata + after as u64) && metadata <= len
}

/// One line into `line` (its end left out), as far as [`LINE`].
struct Next {
    any: bool,
    /// It went on past that.
    cut: bool,
    /// And what is past it has the field's name in it.
    beyond: bool,
    /// Its end is there (the last line of a file cut short has none).
    ended: bool,
}

fn next_line(r: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Next> {
    line.clear();
    let (mut any, mut beyond, mut cut, mut tail) = (false, false, false, Vec::<u8>::new());
    loop {
        let piece = r.fill_buf()?;
        if piece.is_empty() {
            return Ok(Next { any, cut, beyond, ended: false });
        }
        any = true;
        let end = piece.iter().position(|b| *b == b'\n');
        let part = &piece[..end.unwrap_or(piece.len())];
        let room = LINE - line.len();
        line.extend_from_slice(&part[..part.len().min(room)]);
        if part.len() > room {
            if !cut {
                // The name may begin before the cut.
                tail.extend_from_slice(&line[line.len().saturating_sub(FIELD.len())..]);
            }
            cut = true;
            tail.drain(..tail.len().saturating_sub(FIELD.len()));
            tail.extend_from_slice(&part[room..]);
            beyond |= says(&tail, FIELD);
        }
        let used = end.map_or(piece.len(), |e| e + 1);
        r.consume(used);
        if end.is_some() {
            return Ok(Next { any: true, cut, beyond, ended: true });
        }
    }
}

/// Whether a text has the shape of one Unity wrote to its end: documents that begin as its do, each with its object, brackets and
/// quotes that close, and a last line that is whole and is not the name of something that should follow. A text that does not
/// (cut short, merged by hand, not Unity's) may be missing what it had.
#[derive(Default)]
struct Shape {
    documents: usize,
    /// The line after a document's beginning is its object's kind.
    expects_object: bool,
    odd: bool,
    /// Brackets open over lines, and the quote a text is in.
    depth: i64,
    quote: Option<u8>,
    last_is_open: bool,
    ended: bool,
    first: bool,
}

impl Shape {
    fn line(&mut self, line: &[u8], ended: bool) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        self.ended = ended;
        let line = if self.first { line } else { line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line) };
        self.first = true;
        if line.is_empty() || (line[0] == b'%' && self.documents == 0) {
            return;
        }
        self.odd |= std::str::from_utf8(line).is_err() && line.len() < LINE;
        if self.quote.is_none() && self.depth == 0 {
            if line.starts_with(b"--- ") {
                self.odd |= self.expects_object || !line.starts_with(b"--- !u!");
                self.documents += 1;
                self.expects_object = true;
                self.last_is_open = true;
                return;
            }
            if self.expects_object {
                self.odd |= !(line[0].is_ascii_alphabetic() && line.ends_with(b":"));
                self.expects_object = false;
                self.last_is_open = true;
                return;
            }
            // Conflict marks, a tab for an indent, something before the first document.
            let marks: [&[u8]; 4] = [b"<<<<<<<", b"=======", b">>>>>>>", b"\t"];
            self.odd |= self.documents == 0 || marks.iter().any(|m| line.starts_with(m));
        }
        // Where the value is: a quote begins a text only there, a bracket only there.
        let mut at = 0;
        if self.quote.is_none() && self.depth == 0 {
            at = line.iter().position(|b| *b != b' ').unwrap_or(0);
            while line[at..].starts_with(b"- ") {
                at += 2;
            }
            if let Some(colon) = line[at..].windows(2).position(|w| w == b": ") {
                if !matches!(line.get(at), Some(b'{' | b'[' | b'"' | b'\'')) {
                    at += colon + 2;
                }
            }
            if !matches!(line.get(at), Some(b'{' | b'[' | b'"' | b'\'')) {
                // A plain text, which may hold any of them.
                self.last_is_open = line.ends_with(b":");
                return;
            }
        }
        let mut i = at;
        while i < line.len() {
            match (self.quote, line[i]) {
                (Some(b'"'), b'\\') => i += 1,
                // Two of them are one of the text's own.
                (Some(b'\''), b'\'') if line.get(i + 1) == Some(&b'\'') => i += 1,
                (Some(q), b) if q == b => self.quote = None,
                (Some(_), _) => {}
                (None, q @ (b'"' | b'\'')) => self.quote = Some(q),
                (None, b'{' | b'[') => self.depth += 1,
                (None, b'}' | b']') => self.depth -= 1,
                (None, _) => {}
            }
            i += 1;
        }
        self.odd |= self.depth < 0;
        self.last_is_open = false;
    }

    fn finished(&self) -> bool {
        !self.odd && self.documents > 0 && !self.expects_object && self.depth == 0 && self.quote.is_none() && self.ended && !self.last_is_open
    }
}

/// Unity's text: an ID counts where a blueprint ID is (a portal names other worlds), which is a component's own field or a
/// prefab override's value under its property's name. Such a place with something that is not an ID may still be one.
fn text(file: impl Read) -> io::Result<(BTreeSet<String>, bool)> {
    let mut r = BufReader::with_capacity(256 * 1024, file);
    let (mut ids, mut whole, mut property, mut line) = (BTreeSet::new(), true, false, vec![]);
    let mut shape = Shape::default();
    let after = |line: &[u8], word: &[u8]| -> Option<Vec<u8>> {
        let at = line.windows(word.len()).position(|w| w == word)? + word.len();
        Some(line[at..].to_vec())
    };
    loop {
        let next = next_line(&mut r, &mut line)?;
        if !next.any {
            return Ok((ids, whole && shape.finished()));
        }
        shape.line(&line, next.ended);
        if says(&line, b"propertyPath:") {
            property = says(&line, FIELD);
        }
        // Not text at all; or a line too long to be looked at whole that has to do with one.
        whole &= !line.contains(&0) && !(next.cut && (next.beyond || property || says(&line, FIELD)));
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
