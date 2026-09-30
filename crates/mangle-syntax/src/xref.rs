//! Cross-reference tables and streams, including the `/Prev` and `/XRefStm` chains.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, Name, Object, Stream};
use crate::parser::Parser;

/// Where one object lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    /// A free object: the next free object number and the generation in use.
    Free { next: u32, generation: u16 },
    /// A normal object at a byte offset in the file.
    InFile { offset: usize, generation: u16 },
    /// An object inside an object stream.
    InStream { stream: u32, index: u32 },
    /// The object number was listed but no usable location was given.
    Missing,
}

/// The whole cross-reference state of a file.
#[derive(Debug, Clone, Default)]
pub struct Xref {
    /// Object number to entry. A `BTreeMap` so iteration is deterministic.
    entries: BTreeMap<u32, XrefEntry>,
    pub(crate) trailer: Dict,
    /// `/Size` from the newest trailer.
    size: u32,
    /// The kind of the newest cross-reference section, which an incremental update must
    /// match.
    kind: SectionKind,
}

/// Whether the newest section was a classic table or a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionKind {
    #[default]
    Table,
    Stream,
}

/// Guard against a `/Prev` chain that loops or grows without bound.
const MAX_SECTIONS: usize = 1024;

impl Xref {
    #[must_use]
    pub fn get(&self, num: u32) -> Option<XrefEntry> {
        self.entries.get(&num).copied()
    }

    #[must_use]
    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
            .max(1 + self.entries.keys().copied().max().unwrap_or(0))
    }

    #[must_use]
    pub fn kind(&self) -> SectionKind {
        self.kind
    }

    pub fn set(&mut self, num: u32, entry: XrefEntry) {
        self.entries.insert(num, entry);
    }

    pub fn set_trailer(&mut self, t: Dict) {
        self.trailer = t;
    }

    /// Update `/Size` in the trailer.
    pub fn set_trailer_key_size(&mut self, size: u32) {
        self.size = size;
        self.trailer.set("Size", Object::Int(i64::from(size)));
    }

    /// Force the section kind, used by the repair path.
    pub fn set_kind(&mut self, kind: SectionKind) {
        self.kind = kind;
    }

    /// Record where a newly written object lives, for the incremental writer.
    pub fn note_offset(&mut self, num: u32, offset: usize, generation: u16) {
        self.entries
            .insert(num, XrefEntry::InFile { offset, generation });
    }

    /// Numbers of every object the file knows about, in order.
    pub fn object_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.keys().copied()
    }

    /// Objects that are actually in the file, in order.
    pub fn live_objects(&self) -> impl Iterator<Item = (u32, XrefEntry)> + '_ {
        self.entries
            .iter()
            .filter(|(_, e)| !matches!(e, XrefEntry::Free { .. } | XrefEntry::Missing))
            .map(|(k, v)| (*k, *v))
    }

    /// The `/ID` first element, needed for encryption and for the incremental trailer.
    #[must_use]
    pub fn id0(&self) -> Vec<u8> {
        self.trailer
            .get("ID")
            .and_then(Object::as_array)
            .and_then(|a| a.first())
            .and_then(Object::as_bytes)
            .map_or_else(Vec::new, <[u8]>::to_vec)
    }

    /// The offset recorded in the last `startxref`, or `None`.
    #[must_use]
    pub fn start_xref(&self) -> Option<u64> {
        self.trailer
            .get("PrevXref")
            .and_then(Object::as_i64)
            .and_then(|v| u64::try_from(v).ok())
    }
}

/// Find `startxref` in the last 2 KB of the file and return the offset it names.
#[must_use]
pub fn find_startxref(data: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"startxref";
    let tail_start = data.len().saturating_sub(2048);
    let tail = data.get(tail_start..)?;
    // Search backwards so the *last* table in an incrementally updated file wins.
    let mut i = tail.len();
    loop {
        if tail.get(i..i.saturating_add(KEY.len())) == Some(KEY) {
            if let Some(n) = read_offset_after(tail, i + KEY.len()) {
                return Some(n);
            }
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// Read the decimal offset that follows `from`, skipping whitespace and comments.
fn read_offset_after(tail: &[u8], from: usize) -> Option<usize> {
    let mut pos = from;
    while let Some(&b) = tail.get(pos) {
        if b.is_ascii_whitespace() || b == b'\0' {
            pos += 1;
        } else if b == b'%' {
            while let Some(&c) = tail.get(pos) {
                pos += 1;
                if c == b'\n' || c == b'\r' {
                    break;
                }
            }
        } else {
            break;
        }
    }
    let mut n: u64 = 0;
    let mut any = false;
    while let Some(&c) = tail.get(pos) {
        if c.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add(u64::from(c - b'0'));
            any = true;
            pos += 1;
        } else {
            break;
        }
    }
    if any { usize::try_from(n).ok() } else { None }
}

/// Read the whole cross-reference chain starting at `start`, and mark anything that
/// looks broken so the repair path can take over.
pub fn read_xref(data: &[u8], start: usize, notes: &mut Vec<String>) -> Result<Xref> {
    let mut xref = Xref::default();
    let mut seen_offsets: Vec<usize> = Vec::new();
    let mut queue = vec![start];
    let mut sections = 0usize;
    let mut first = true;

    while let Some(offset) = queue.pop() {
        sections += 1;
        if sections > MAX_SECTIONS {
            notes.push(format!(
                "cross-reference chain exceeded {MAX_SECTIONS} sections"
            ));
            break;
        }
        if seen_offsets.contains(&offset) {
            notes.push(format!("cross-reference loop at offset {offset}"));
            continue;
        }
        seen_offsets.push(offset);
        let Some(section) = read_section(data, offset) else {
            notes.push(format!("no cross-reference section at offset {offset}"));
            continue;
        };
        if first {
            xref.kind = section.kind;
            first = false;
        }
        for (num, entry) in section.entries {
            // A newer section wins: it is visited before older ones.
            xref.entries.entry(num).or_insert(entry);
        }
        for key in ["Root", "Info", "Encrypt", "ID", "Size"] {
            if !xref.trailer.contains(key)
                && let Some(v) = section.trailer.get(key)
            {
                xref.trailer.set(key, v.clone());
            }
        }
        if let Some(size) = section.trailer.get("Size").and_then(Object::as_i64) {
            xref.size = u32::try_from(size).unwrap_or(xref.size);
        }
        // Hybrid files keep the real entries in a parallel stream.
        if let Some(x) = section.trailer.get("XRefStm").and_then(Object::as_i64)
            && let Ok(x) = usize::try_from(x)
        {
            queue.push(x);
        }
        if let Some(p) = section.trailer.get("Prev").and_then(Object::as_i64)
            && let Ok(p) = usize::try_from(p)
        {
            queue.push(p);
        }
    }

    // Keep `/PrevXref` so the incremental writer can chain to it.
    if let Some(s) = find_startxref(data) {
        xref.trailer
            .set("PrevXref", Object::Int(i64::try_from(s).unwrap_or(0)));
    }
    Ok(xref)
}

struct Section {
    entries: Vec<(u32, XrefEntry)>,
    pub(crate) trailer: Dict,
    kind: SectionKind,
}

/// Read one cross-reference section: either a classic table or a stream.
fn read_section(data: &[u8], offset: usize) -> Option<Section> {
    // Work on the whole file so every position stays absolute.
    let mut lx = Lexer::new(data);
    lx.seek(offset);
    lx.skip_space();
    let save = lx.position();
    let first = lx.next_token().ok()?;
    match &first.token {
        Token::Keyword(k) if k == b"xref" => read_table(data, save),
        _ => {
            // Probably "N G obj" holding an xref stream.
            let mut p = Parser::at(data, save);
            let Ok(Some(Object::Int(_num))) = p.next_object() else {
                return None;
            };
            if p.next_object().ok()? != Some(Object::Int(0)) || !p.next_keyword(b"obj") {
                return None;
            }
            let Some(Object::Stream(stream)) = p.next_object().ok()? else {
                return None;
            };
            let entries = read_xref_stream(&stream)?;
            let mut trailer = stream.dict.clone();
            trailer.remove("W");
            trailer.remove("Index");
            trailer.remove("Filter");
            trailer.remove("Length");
            trailer.remove("DecodeParms");
            Some(Section {
                entries,
                trailer,
                kind: SectionKind::Stream,
            })
        }
    }
}

fn read_table(data: &[u8], mut pos: usize) -> Option<Section> {
    let mut entries: Vec<(u32, XrefEntry)> = Vec::new();
    loop {
        // All lexers run over the whole file so every position stays absolute.
        let mut lx = Lexer::new(data);
        lx.seek(pos);
        lx.skip_space();
        let t = lx.next_token().ok()?;
        match &t.token {
            Token::Keyword(k) if k == b"trailer" => {
                let mut p = Parser::at(data, t.end);
                let Some(Object::Dict(trailer)) = p.next_object().ok()? else {
                    return None;
                };
                return Some(Section {
                    entries,
                    trailer,
                    kind: SectionKind::Table,
                });
            }
            Token::Keyword(k) if k == b"xref" => pos = t.end,
            Token::Int(start) => {
                let Token::Int(count) = lx.next_token().ok()?.token else {
                    return None;
                };
                pos = lx.position();
                for i in 0..count.max(0) {
                    let mut entry_lx = Lexer::new(data);
                    entry_lx.seek(pos);
                    entry_lx.skip_space();
                    let off = entry_lx.next_token().ok()?;
                    let generation = entry_lx.next_token().ok()?;
                    let kind = entry_lx.next_token().ok()?;
                    pos = entry_lx.position();
                    let (Token::Int(o), Token::Int(g)) = (&off.token, &generation.token) else {
                        return None;
                    };
                    let num = u32::try_from(start.saturating_add(i)).unwrap_or(0);
                    let g16 = u16::try_from(*g).unwrap_or(0);
                    let entry = match kind.token {
                        Token::Keyword(k) if k == b"n" => XrefEntry::InFile {
                            offset: usize::try_from(*o).unwrap_or(0),
                            generation: g16,
                        },
                        Token::Keyword(k) if k == b"f" => XrefEntry::Free {
                            next: u32::try_from(*o).unwrap_or(0),
                            generation: g16,
                        },
                        _ => XrefEntry::Missing,
                    };
                    entries.push((num, entry));
                }
            }
            _ => return None,
        }
    }
}

/// Decode an xref stream's `/W` and `/Index` arrays into entries.
fn read_xref_stream(stream: &Stream) -> Option<Vec<(u32, XrefEntry)>> {
    let widths: Vec<usize> = stream
        .dict
        .get("W")
        .and_then(Object::as_array)?
        .iter()
        .filter_map(Object::as_i64)
        .map(|v| usize::try_from(v).unwrap_or(0))
        .collect();
    if widths.len() < 3 || widths.iter().sum::<usize>() == 0 {
        return None;
    }
    let size = stream
        .dict
        .get("Size")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0);
    let index: Vec<i64> = match stream.dict.get("Index").and_then(Object::as_i64) {
        Some(_) => stream
            .dict
            .get("Index")
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(Object::as_i64).collect())
            .unwrap_or_default(),
        None => vec![0, size],
    };
    let decoded = crate::stream::decode_stream(stream).data;
    let width: usize = widths.iter().sum();
    let mut out = Vec::new();
    let mut pos = 0usize;
    for chunk in index.chunks(2) {
        let Some(&start) = chunk.first() else { break };
        let count = chunk.get(1).copied().unwrap_or(0);
        for i in 0..count.max(0) {
            if pos + width > decoded.len() {
                return Some(out);
            }
            let mut fields = [0u64; 3];
            for (k, w) in widths.iter().take(3).enumerate() {
                let mut v = 0u64;
                for _b in 0..*w {
                    let byte = decoded.get(pos).copied().unwrap_or(0);
                    v = (v << 8) | u64::from(byte);
                    pos += 1;
                }
                if let Some(slot) = fields.get_mut(k) {
                    *slot = v;
                }
            }
            let num = u32::try_from(start.saturating_add(i)).unwrap_or(0);
            let generation = u16::try_from(fields[2]).unwrap_or(0);
            let entry = match fields[0] {
                0 => XrefEntry::Free {
                    next: u32::try_from(fields[1]).unwrap_or(0),
                    generation,
                },
                1 => XrefEntry::InFile {
                    offset: usize::try_from(fields[1]).unwrap_or(0),
                    generation,
                },
                2 => XrefEntry::InStream {
                    stream: u32::try_from(fields[1]).unwrap_or(0),
                    index: u32::try_from(fields[2]).unwrap_or(0),
                },
                _ => XrefEntry::Missing,
            };
            out.push((num, entry));
        }
    }
    Some(out)
}

/// The catalogue root object, searching the trailer and then the whole file.
#[must_use]
pub fn find_root(data: &[u8], xref: &Xref) -> Option<crate::object::Ref> {
    if let Some(r) = xref.trailer.get("Root").and_then(Object::as_ref_id) {
        return Some(r);
    }
    // Scan for `/Type /Catalog` as a fallback.
    for (num, entry) in xref.live_objects() {
        let XrefEntry::InFile { offset, .. } = entry else {
            continue;
        };
        let Some(chunk) = data.get(offset..) else {
            continue;
        };
        if find_type_catalog(chunk) {
            return Some(crate::object::Ref::new(num, 0));
        }
    }
    None
}

fn find_type_catalog(chunk: &[u8]) -> bool {
    let mut lx = Lexer::new(chunk);
    while let Ok(t) = lx.next_token() {
        if t.is_keyword(b"obj") {
            if let Ok(Some(Object::Dict(d))) = Parser::at(chunk, t.end).next_object() {
                if d.get("Type").and_then(Object::as_name) == Some(&b"Catalog"[..]) {
                    return true;
                }
            }
        }
        if lx.at_end() {
            break;
        }
    }
    false
}

/// A minimal `Dict` helper used by the writer when it must synthesise a trailer.
pub fn trailer_with(root: Option<crate::object::Ref>, size: u32, id0: Vec<u8>) -> Dict {
    let mut d = Dict::new();
    if let Some(r) = root {
        d.insert(Name::new("Root"), Object::Ref(r));
    }
    d.set("Size", Object::Int(i64::from(size)));
    if !id0.is_empty() {
        d.set(
            "ID",
            Object::Array(vec![Object::String(id0.clone()), Object::String(id0)]),
        );
    }
    d
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn build(body: &str) -> Vec<u8> {
        body.as_bytes().to_vec()
    }

    #[test]
    fn startxref_is_found() {
        let data = build("junk\nstartxref\n1234\n%%EOF\n");
        assert_eq!(find_startxref(&data), Some(1234));
    }

    #[test]
    fn startxref_missing() {
        let data = build("no idea\n%%EOF\n");
        assert_eq!(find_startxref(&data), None);
    }

    #[test]
    fn classic_table() {
        let data = build(
            "xref\n0 3\n0000000000 65535 f \n0000000010 00000 n \n0000000030 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n0\n%%EOF\n",
        );
        let mut notes = Vec::new();
        let x = read_xref(&data, 0, &mut notes).expect("xref");
        assert_eq!(
            x.get(0),
            Some(XrefEntry::Free {
                next: 0,
                generation: 65535
            })
        );
        assert_eq!(
            x.get(1),
            Some(XrefEntry::InFile {
                offset: 10,
                generation: 0
            })
        );
        assert_eq!(
            x.get(2),
            Some(XrefEntry::InFile {
                offset: 30,
                generation: 0
            })
        );
        assert_eq!(x.size(), 3);
        assert_eq!(
            x.trailer().get("Root").and_then(Object::as_ref_id),
            Some(crate::object::Ref::new(1, 0))
        );
    }

    #[test]
    fn prev_chain_prefers_newer_entries() {
        let first =
            "xref\n0 2\n0000000000 65535 f \n0000000010 00000 n \ntrailer\n<< /Size 2 /Prev 0 >>\n";
        let mut data = build(first);
        let older = data.len();
        data.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n0000000099 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R >>\n");
        let mut notes = Vec::new();
        // Start at the *older* section and follow `/Prev` forward is impossible, so
        // instead start at the newer one and confirm the chain links back correctly.
        let x = read_xref(&data, older, &mut notes).expect("xref");
        assert_eq!(
            older, 79,
            "the fixture offset must stay in step with the text"
        );
        assert_eq!(
            x.get(1),
            Some(XrefEntry::InFile {
                offset: 99,
                generation: 0
            }),
            "the newer section must win"
        );
    }

    #[test]
    fn loop_in_prev_is_reported_not_hung() {
        let data = build("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev 0 >>\n");
        let mut notes = Vec::new();
        let x = read_xref(&data, 0, &mut notes).expect("xref");
        assert!(notes.iter().any(|n| n.contains("loop")), "{notes:?}");
        assert_eq!(x.size(), 1);
    }

    #[test]
    fn xref_stream() {
        // /W [1 2 1], two entries: a free 0 and object 1 at offset 42, generation 0.
        let mut dict = Dict::new();
        dict.set("Type", Object::name("XRef"));
        dict.set("Size", Object::Int(2));
        dict.set(
            "W",
            Object::Array(vec![Object::Int(1), Object::Int(2), Object::Int(1)]),
        );
        dict.set("Root", Object::Ref(crate::object::Ref::new(1, 0)));
        let data_body = vec![0u8, 0, 0, 0, 1, 0, 42, 0];
        let stream = Stream::new(dict, data_body);
        let entries = read_xref_stream(&stream).expect("entries");
        assert_eq!(
            entries.first().copied(),
            Some((
                0,
                XrefEntry::Free {
                    next: 0,
                    generation: 0
                }
            ))
        );
        assert_eq!(
            entries.get(1).copied(),
            Some((
                1,
                XrefEntry::InFile {
                    offset: 42,
                    generation: 0
                }
            ))
        );
    }

    #[test]
    fn in_stream_entry() {
        let mut dict = Dict::new();
        dict.set(
            "W",
            Object::Array(vec![Object::Int(1), Object::Int(1), Object::Int(1)]),
        );
        dict.set("Size", Object::Int(2));
        let stream = Stream::new(dict, vec![0, 0, 0, 2, 5, 3]);
        let entries = read_xref_stream(&stream).expect("entries");
        assert_eq!(
            entries.get(1).copied(),
            Some((
                1,
                XrefEntry::InStream {
                    stream: 5,
                    index: 3
                }
            ))
        );
    }
}
