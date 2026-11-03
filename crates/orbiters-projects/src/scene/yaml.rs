//! Unity's text serialization (scenes, prefabs, materials, `.meta` files): a YAML subset Unity writes the same way
//! every time. A file is a list of documents, each `--- !u!<class ID> &<file ID>` (and ` stripped` for an object that
//! only stands for one in a prefab instance) over one object; inside, two-space block mappings, sequences whose dashes
//! sit at their key's own indentation, flow mappings for references and vectors (`{fileID: 0, guid: …, type: 3}`,
//! `{x: 0, y: 1, z: 0}`) that may wrap onto the next lines, and plain scalars that Unity also wraps when long.
//!
//! A general YAML library would choke on the `!u!` tags and is far slower than needed on multi-megabyte avatar
//! prefabs: this reads only that subset, and only the documents asked for ([`documents`] splits a file without
//! parsing it). Anything it does not understand becomes a string or is skipped; it never panics on odd input.

use std::borrow::Cow;

/// One object of a Unity file, not parsed yet.
#[derive(Clone, Copy, Debug)]
pub struct Doc<'a> {
    pub class: u32,
    pub id: i64,
    /// Stands for an object of a prefab instance (only its links to it are written).
    pub stripped: bool,
    /// Everything after the document's header line: the object's type name, then its fields.
    pub body: &'a str,
}

impl<'a> Doc<'a> {
    /// The object's fields (under its type name).
    pub fn parse(&self) -> Value {
        match parse(self.body) {
            Value::Map(mut m) if m.len() == 1 => m.pop().map(|(_, v)| v).unwrap_or(Value::Null),
            v => v,
        }
    }

    /// Whether the body holds `needle` (a quick test before parsing).
    pub fn mentions(&self, needle: &str) -> bool {
        self.body.contains(needle)
    }
}

/// Whether `text` is a Unity text file (an asset saved in binary starts otherwise).
pub fn is_text(text: &str) -> bool {
    text.starts_with("%YAML") || text.starts_with("--- !u!")
}

/// The documents of a Unity file, in order. Lines before the first document (`%YAML`, `%TAG`) are skipped.
pub fn documents(text: &str) -> Vec<Doc<'_>> {
    let mut out = Vec::new();
    let mut start: Option<(u32, i64, bool, usize)> = None;
    let mut pos = 0;
    let bytes = text.as_bytes();
    while pos < bytes.len() {
        let end = memchr_newline(bytes, pos);
        let line = &text[pos..end];
        if line.starts_with("--- !u!") {
            if let Some((class, id, stripped, body)) = start.take() {
                out.push(Doc { class, id, stripped, body: &text[body..pos] });
            }
            start = header(line).map(|(c, i, s)| (c, i, s, (end + 1).min(text.len())));
        }
        pos = end + 1;
    }
    if let Some((class, id, stripped, body)) = start {
        out.push(Doc { class, id, stripped, body: &text[body.min(text.len())..] });
    }
    out
}

fn memchr_newline(bytes: &[u8], from: usize) -> usize {
    bytes[from..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |p| from + p)
}

/// `--- !u!4 &123 stripped` → (4, 123, true).
fn header(line: &str) -> Option<(u32, i64, bool)> {
    let rest = line.strip_prefix("--- !u!")?;
    let mut parts = rest.split_whitespace();
    let class = parts.next()?.parse().ok()?;
    let id = parts.next()?.strip_prefix('&')?.parse().ok()?;
    let stripped = parts.any(|p| p == "stripped");
    Some((class, id, stripped))
}

/// A parsed value: Unity writes only strings, mappings (in order) and sequences.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Null,
    Str(String),
    Map(Vec<(String, Value)>),
    Seq(Vec<Value>),
    /// Raw bytes (a binary serialized file's byte arrays; see [`super::serialized`]).
    Bytes(Vec<u8>),
}

/// A reference to an object: in the same file (`guid` none) or in the asset whose GUID it names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ref {
    pub file_id: i64,
    pub guid: Option<String>,
}

impl Ref {
    pub fn is_null(&self) -> bool {
        self.file_id == 0 && self.guid.is_none()
    }
}

static NULL: Value = Value::Null;

impl Value {
    /// A mapping's value at `key` (`Null` when it is not one or has none).
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k == key).map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }

    /// Follows a path of keys.
    pub fn at(&self, path: &[&str]) -> &Value {
        path.iter().fold(self, |v, k| v.get(k))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn str_or<'a>(&'a self, otherwise: &'a str) -> &'a str {
        self.as_str().unwrap_or(otherwise)
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_str().and_then(|s| s.trim().parse::<f32>().ok()).filter(|v| v.is_finite())
    }

    pub fn f32_or(&self, otherwise: f32) -> f32 {
        self.as_f32().unwrap_or(otherwise)
    }

    pub fn as_i64(&self) -> Option<i64> {
        let s = self.as_str()?.trim();
        s.parse::<i64>().ok().or_else(|| s.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v as i64))
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_i64().map(|v| v != 0)
    }

    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Map(m) => m,
            _ => &[],
        }
    }

    pub fn items(&self) -> &[Value] {
        match self {
            Value::Seq(s) => s,
            _ => &[],
        }
    }

    /// `{fileID: …, guid: …}` as a reference.
    pub fn as_ref(&self) -> Option<Ref> {
        let file_id = self.get("fileID").as_i64()?;
        let guid = self.get("guid").as_str().filter(|g| !g.is_empty() && g.bytes().any(|b| b != b'0')).map(str::to_ascii_lowercase);
        Some(Ref { file_id, guid })
    }

    /// `{x: …, y: …, z: …}`.
    pub fn vec3(&self) -> Option<[f32; 3]> {
        Some([self.get("x").as_f32()?, self.get("y").as_f32()?, self.get("z").as_f32()?])
    }

    /// `{x: …, y: …, z: …, w: …}`.
    pub fn quat(&self) -> Option<[f32; 4]> {
        Some([self.get("x").as_f32()?, self.get("y").as_f32()?, self.get("z").as_f32()?, self.get("w").as_f32()?])
    }

    /// `{r: …, g: …, b: …, a: …}`.
    pub fn color(&self) -> Option<[f32; 4]> {
        Some([self.get("r").as_f32()?, self.get("g").as_f32()?, self.get("b").as_f32()?, self.get("a").as_f32().unwrap_or(1.0)])
    }

    /// `{x: …, y: …}`.
    pub fn vec2(&self) -> Option<[f32; 2]> {
        Some([self.get("x").as_f32()?, self.get("y").as_f32()?])
    }
}

struct Line<'a> {
    indent: usize,
    text: Cow<'a, str>,
}

/// Parses a block of Unity YAML (a document's body, a `.meta` file).
pub fn parse(text: &str) -> Value {
    let mut lines: Vec<Line> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            let t = l.trim_start_matches(' ');
            if t.is_empty() || t.starts_with('#') || t.starts_with('%') || t.starts_with("--- ") {
                return None;
            }
            Some(Line { indent: l.len() - t.len(), text: Cow::Borrowed(t) })
        })
        .collect();
    let mut i = 0;
    let Some(first) = lines.first() else { return Value::Null };
    let indent = first.indent;
    block(&mut lines, &mut i, indent, 0)
}

const MAX_DEPTH: usize = 64;

fn is_item(t: &str) -> bool {
    t == "-" || t.starts_with("- ")
}

fn block(lines: &mut [Line], i: &mut usize, indent: usize, depth: usize) -> Value {
    if *i >= lines.len() || depth > MAX_DEPTH {
        return Value::Null;
    }
    if is_item(&lines[*i].text) {
        sequence(lines, i, indent, depth)
    } else {
        mapping(lines, i, indent, depth)
    }
}

fn sequence(lines: &mut [Line], i: &mut usize, indent: usize, depth: usize) -> Value {
    let mut out = Vec::new();
    while *i < lines.len() && lines[*i].indent == indent && is_item(&lines[*i].text) {
        let rest = lines[*i].text.get(1..).unwrap_or("").trim_start_matches(' ').to_string();
        let gap = lines[*i].text.len() - 1 - rest.len();
        if rest.is_empty() {
            *i += 1;
            if *i < lines.len() && lines[*i].indent > indent {
                let inner = lines[*i].indent;
                out.push(block(lines, i, inner, depth + 1));
            } else {
                out.push(Value::Null);
            }
        } else if is_item(&rest) || key_split(&rest).is_some() {
            // `- key: value` (and further keys under it): a mapping whose first line is this one.
            let inner = indent + 1 + gap;
            lines[*i] = Line { indent: inner, text: Cow::Owned(rest) };
            out.push(block(lines, i, inner, depth + 1));
        } else {
            *i += 1;
            out.push(scalar_with_continuation(lines, i, indent, &rest));
        }
    }
    Value::Seq(out)
}

fn mapping(lines: &mut [Line], i: &mut usize, indent: usize, depth: usize) -> Value {
    let mut out = Vec::new();
    while *i < lines.len() && lines[*i].indent == indent && !is_item(&lines[*i].text) {
        let Some((key, value)) = key_split(&lines[*i].text).map(|(k, v)| (k.to_string(), v.to_string())) else {
            // Not a key (a stray line): skipped.
            *i += 1;
            continue;
        };
        *i += 1;
        let v = if value.is_empty() {
            if *i < lines.len() && lines[*i].indent > indent {
                let inner = lines[*i].indent;
                block(lines, i, inner, depth + 1)
            } else if *i < lines.len() && lines[*i].indent == indent && is_item(&lines[*i].text) {
                // A sequence written at its key's indentation.
                sequence(lines, i, indent, depth + 1)
            } else {
                Value::Null
            }
        } else {
            scalar_with_continuation(lines, i, indent, &value)
        };
        out.push((key, v));
    }
    // Lines deeper than this mapping that no key took (odd input): skipped, so that the caller goes on.
    while *i < lines.len() && lines[*i].indent > indent {
        *i += 1;
    }
    Value::Map(out)
}

/// A value written after a key or a dash, with the lines that continue it: a flow collection until its brackets
/// close, a plain or quoted scalar Unity wrapped onto deeper lines.
fn scalar_with_continuation(lines: &[Line], i: &mut usize, indent: usize, first: &str) -> Value {
    let mut text = first.to_string();
    if text.starts_with('{') || text.starts_with('[') {
        while !balanced(&text) && *i < lines.len() && lines[*i].indent > indent {
            text.push(' ');
            text.push_str(&lines[*i].text);
            *i += 1;
        }
        return flow(&text);
    }
    let quoted = text.starts_with('"') || text.starts_with('\'');
    while *i < lines.len() && lines[*i].indent > indent && (quoted || !is_item(&lines[*i].text)) {
        if quoted && closed_quote(&text) {
            break;
        }
        text.push(' ');
        text.push_str(&lines[*i].text);
        *i += 1;
    }
    Value::Str(unquote(&text))
}

fn closed_quote(t: &str) -> bool {
    let q = t.as_bytes()[0];
    if t.len() < 2 {
        return false;
    }
    if q == b'\'' {
        // '' inside is an escaped quote: an odd count of quotes overall closes it.
        return t.ends_with('\'') && t.bytes().filter(|&b| b == b'\'').count() % 2 == 0;
    }
    t.ends_with('"') && !t.ends_with("\\\"")
}

/// `key: value` → (key, value); `key:` → (key, ""). None for a line without a key.
fn key_split(t: &str) -> Option<(&str, &str)> {
    if t.starts_with('{') || t.starts_with('[') || t.starts_with('"') || t.starts_with('\'') {
        return None;
    }
    let bytes = t.as_bytes();
    let mut k = 0;
    while k < bytes.len() {
        if bytes[k] == b':' && (k + 1 == bytes.len() || bytes[k + 1] == b' ') {
            let key = t[..k].trim_end();
            if key.is_empty() {
                return None;
            }
            return Some((key, t[k + 1..].trim()));
        }
        k += 1;
    }
    None
}

fn balanced(t: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for b in t.bytes() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b'"' | b'\'' => quote = Some(b),
                _ => {}
            },
        }
    }
    depth <= 0
}

/// A flow collection (`{a: 1, b: {c: 2}}`, `[1, 2]`) or a scalar.
pub fn flow(t: &str) -> Value {
    let bytes = t.as_bytes();
    let mut pos = 0;
    flow_value(t, bytes, &mut pos, 0)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && (bytes[*pos] == b' ' || bytes[*pos] == b'\t' || bytes[*pos] == b'\n' || bytes[*pos] == b'\r') {
        *pos += 1;
    }
}

fn flow_value(t: &str, bytes: &[u8], pos: &mut usize, depth: usize) -> Value {
    skip_ws(bytes, pos);
    if *pos >= bytes.len() || depth > MAX_DEPTH {
        return Value::Null;
    }
    match bytes[*pos] {
        b'{' => {
            *pos += 1;
            let mut out = Vec::new();
            loop {
                skip_ws(bytes, pos);
                if *pos >= bytes.len() {
                    break;
                }
                if bytes[*pos] == b'}' {
                    *pos += 1;
                    break;
                }
                if bytes[*pos] == b',' {
                    *pos += 1;
                    continue;
                }
                let key = flow_scalar(t, bytes, pos, true);
                skip_ws(bytes, pos);
                let value = if *pos < bytes.len() && bytes[*pos] == b':' {
                    *pos += 1;
                    flow_value(t, bytes, pos, depth + 1)
                } else {
                    Value::Null
                };
                out.push((key, value));
            }
            Value::Map(out)
        }
        b'[' => {
            *pos += 1;
            let mut out = Vec::new();
            loop {
                skip_ws(bytes, pos);
                if *pos >= bytes.len() {
                    break;
                }
                if bytes[*pos] == b']' {
                    *pos += 1;
                    break;
                }
                if bytes[*pos] == b',' {
                    *pos += 1;
                    continue;
                }
                out.push(flow_value(t, bytes, pos, depth + 1));
            }
            Value::Seq(out)
        }
        _ => {
            let s = flow_scalar(t, bytes, pos, false);
            Value::Str(s)
        }
    }
}

/// A scalar inside a flow collection: up to `,` `}` `]` (and `: ` for a key), quotes honoured.
fn flow_scalar(t: &str, bytes: &[u8], pos: &mut usize, key: bool) -> String {
    skip_ws(bytes, pos);
    let start = *pos;
    if *pos < bytes.len() && (bytes[*pos] == b'"' || bytes[*pos] == b'\'') {
        let q = bytes[*pos];
        *pos += 1;
        while *pos < bytes.len() {
            if bytes[*pos] == b'\\' && q == b'"' {
                *pos += 2;
                continue;
            }
            if bytes[*pos] == q {
                if q == b'\'' && *pos + 1 < bytes.len() && bytes[*pos + 1] == b'\'' {
                    *pos += 2;
                    continue;
                }
                *pos += 1;
                break;
            }
            *pos += 1;
        }
        return unquote(&t[start..(*pos).min(t.len())]);
    }
    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b',' || b == b'}' || b == b']' {
            break;
        }
        if key && b == b':' {
            break;
        }
        if !key && b == b':' && (*pos + 1 >= bytes.len() || bytes[*pos + 1] == b' ') {
            break;
        }
        *pos += 1;
    }
    t[start..*pos].trim().to_string()
}

/// A scalar's text: quotes removed and their escapes undone.
fn unquote(t: &str) -> String {
    let t = t.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        return t[1..t.len() - 1].replace("''", "'");
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(' ') => {}
                Some(o) => out.push(o),
                None => {}
            }
        }
        return out;
    }
    t.to_string()
}

/// The GUID a `.meta` file gives its asset (its second line, `guid: …`), read from the file's start only.
pub fn meta_guid(meta: &str) -> Option<String> {
    meta.lines()
        .take(8)
        .find_map(|l| l.strip_prefix("guid:"))
        .map(|g| g.trim().to_ascii_lowercase())
        .filter(|g| g.len() == 32 && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFAB: &str = "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_ObjectHideFlags: 0
  serializedVersion: 6
  m_Component:
  - component: {fileID: 400}
  - component: {fileID: 2300}
  m_Layer: 0
  m_Name: 'Body: it''s here'
  m_TagString: Untagged
  m_IsActive: 1
--- !u!4 &400
Transform:
  m_GameObject: {fileID: 100}
  m_LocalRotation: {x: -0.0006741867, y: 0, z: -0.73743004, w: -0.67542326}
  m_LocalPosition: {x: 1, y: 2.5, z: -3}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 0}
--- !u!1001 &900 stripped
PrefabInstance:
  m_Modification:
    m_Modifications:
    - target: {fileID: 253038329609796467, guid: 9D3ACAF980F650B4B8FF91E72DAD3A02,
        type: 3}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {fileID: 2100000, guid: 0123456789abcdef0123456789abcdef, type: 2}
    - target: {fileID: 7, guid: 9d3acaf980f650b4b8ff91e72dad3a02, type: 3}
      propertyPath: m_Name
      value: A long name that Unity wrapped
        onto the next line
      objectReference: {fileID: 0}
    m_RemovedComponents: []
  m_SourcePrefab: {fileID: 100100000, guid: 9d3acaf980f650b4b8ff91e72dad3a02, type: 3}
";

    #[test]
    fn documents_split_with_their_class_id_and_stripped_mark() {
        let docs = documents(PREFAB);
        assert_eq!(docs.len(), 3);
        assert_eq!((docs[0].class, docs[0].id, docs[0].stripped), (1, 100, false));
        assert_eq!((docs[2].class, docs[2].id, docs[2].stripped), (1001, 900, true));
        assert!(docs[1].body.starts_with("Transform:"));
    }

    #[test]
    fn fields_references_vectors_and_wrapped_values_read_back() {
        let docs = documents(PREFAB);
        let go = docs[0].parse();
        assert_eq!(go.get("m_Name").as_str(), Some("Body: it's here"));
        let comps: Vec<i64> = go.get("m_Component").items().iter().filter_map(|c| c.get("component").as_ref()).map(|r| r.file_id).collect();
        assert_eq!(comps, [400, 2300]);
        assert_eq!(go.get("m_Layer").as_i64(), Some(0));
        let t = docs[1].parse();
        assert_eq!(t.get("m_LocalPosition").vec3(), Some([1.0, 2.5, -3.0]));
        assert_eq!(t.get("m_LocalRotation").quat().unwrap()[3], -0.67542326);
        assert_eq!(t.get("m_Children").items().len(), 0);
        assert!(t.get("m_Father").as_ref().unwrap().is_null());
        let pi = docs[2].parse();
        let mods = pi.at(&["m_Modification", "m_Modifications"]).items();
        assert_eq!(mods.len(), 2);
        let target = mods[0].get("target").as_ref().unwrap();
        assert_eq!(target, Ref { file_id: 253038329609796467, guid: Some("9d3acaf980f650b4b8ff91e72dad3a02".into()) });
        assert_eq!(mods[0].get("propertyPath").as_str(), Some("m_Materials.Array.data[0]"));
        assert_eq!(mods[0].get("value").as_str().unwrap_or(""), "");
        assert_eq!(mods[0].get("objectReference").as_ref().unwrap().guid.as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(mods[1].get("value").as_str(), Some("A long name that Unity wrapped onto the next line"));
        assert_eq!(pi.get("m_SourcePrefab").as_ref().unwrap().file_id, 100100000);
    }

    #[test]
    fn material_properties_as_unity_writes_them() {
        let mat = "Material:
  m_Name: Body
  m_Shader: {fileID: 4800000, guid: df12117ecd77c31469c224178886498e, type: 3}
  m_ValidKeywords:
  - _EMISSION
  m_SavedProperties:
    serializedVersion: 3
    m_TexEnvs:
    - _MainTex:
        m_Texture: {fileID: 2800000, guid: aaaabbbbccccddddeeeeffff00001111, type: 3}
        m_Scale: {x: 2, y: 1}
        m_Offset: {x: 0, y: 0.5}
    m_Floats:
    - _Cutoff: 0.5
    - _Cull: 2
    m_Colors:
    - _Color: {r: 1, g: 0.5, b: 0.25, a: 1}
";
        let v = parse(mat);
        let m = v.get("Material");
        assert_eq!(m.get("m_ValidKeywords").items()[0].as_str(), Some("_EMISSION"));
        let tex = &m.at(&["m_SavedProperties", "m_TexEnvs"]).items()[0];
        assert_eq!(tex.at(&["_MainTex", "m_Scale"]).vec2(), Some([2.0, 1.0]));
        let floats = m.at(&["m_SavedProperties", "m_Floats"]).items();
        assert_eq!(floats[1].get("_Cull").as_f32(), Some(2.0));
        assert_eq!(m.at(&["m_SavedProperties", "m_Colors"]).items()[0].get("_Color").color(), Some([1.0, 0.5, 0.25, 1.0]));
    }

    #[test]
    fn odd_input_never_panics() {
        for text in ["", "---", "--- !u!", "--- !u!x &y\nfoo", "a:\n  - \n  -\n- b", "k: {unclosed: [1, 2", "k: 'open", "  - - - x", ":::", "k: \"\\u12\""] {
            let _ = parse(text);
            let _ = documents(text);
            for d in documents(text) {
                let _ = d.parse();
            }
        }
        assert_eq!(flow("{x: 1, y: {z: 2}}").at(&["y", "z"]).as_i64(), Some(2));
    }

    #[test]
    fn meta_files_give_their_guid() {
        assert_eq!(
            meta_guid("fileFormatVersion: 2\nguid: 91B100EB3BC320843A1BA20C380EA3A2\nModelImporter:\n").as_deref(),
            Some("91b100eb3bc320843a1ba20c380ea3a2")
        );
        assert_eq!(meta_guid("fileFormatVersion: 2\nguid: nope\n"), None);
    }
}
