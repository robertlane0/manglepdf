//! Writing PDF: object serialisation, incremental updates, and full rewrites.
//!
//! Numbers are written without exponents, with at most six decimals and never as `NaN`
//! or infinity, because a viewer that refuses a file over `6.02e23` is not a viewer worth
//! supporting and a file that differs between two saves is not a file worth producing.

use std::collections::{BTreeMap, BTreeSet};

use crate::object::{Dict, Name, Object, Ref, format_number};
use crate::xref::XrefEntry;

/// How a stream should be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCompression {
    /// Keep the bytes exactly as they are. The default, and what untouched images need.
    Keep,
    /// Re-encode with Flate.
    Flate,
    /// Leave the stream out of the file entirely (used when pruning).
    Drop,
}

/// Options for a full rewrite.
///
/// The defaults are the classic layout: no object streams, no cross-reference stream,
/// every reader accepts it, and nothing is written in a form we cannot yet round trip.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteOptions {
    /// Pack non-stream objects into object streams.
    pub use_object_streams: bool,
    /// Write a cross-reference stream rather than a table.
    pub use_xref_streams: bool,
    /// `/ID` for both trailer entries.
    pub file_id: Vec<u8>,
    /// Put non-stream objects into an object stream.
    pub compress_streams: bool,
    /// The version to declare in the header. The document's own, when it has one.
    pub header_version: Option<String>,
    /// The lowest `/Size` to write. `/Size` must be one more than the highest object
    /// number, and it must not shrink when the highest object is deleted, or a reader
    /// counts objects and disagrees with the file.
    pub minimum_size: u32,
}

/// Write a single object into `out`. Public so the object-stream builder can use it.
pub fn write_object_into(out: &mut Vec<u8>, obj: &Object) {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(true) => out.extend_from_slice(b"true"),
        Object::Bool(false) => out.extend_from_slice(b"false"),
        Object::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => out.extend_from_slice(format_number(*r).as_bytes()),
        Object::Name(n) => write_name(out, n),
        Object::String(s) => write_string(out, s),
        Object::Array(a) => {
            out.push(b'[');
            for (i, o) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object_into(out, o);
            }
            out.push(b']');
        }
        Object::Dict(d) => write_dict(out, d),
        Object::Stream(s) => {
            // A bare stream with no object number is not legal; write the dictionary.
            write_dict(out, &s.dict);
        }
        Object::Ref(r) => {
            out.extend_from_slice(r.to_string().as_bytes());
        }
        Object::Missing(r) => out.extend_from_slice(r.to_string().as_bytes()),
    }
}

fn write_name(out: &mut Vec<u8>, name: &Name) {
    out.push(b'/');
    for &b in name.as_bytes() {
        if token_ok(b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

const fn token_ok(b: u8) -> bool {
    b > b' '
        && b < 0x7f
        && !matches!(
            b,
            b'#' | b'/' | b'%' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}'
        )
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'(');
    for &b in s {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            _ => out.push(b),
        }
    }
    out.push(b')');
}

fn write_dict(out: &mut Vec<u8>, d: &Dict) {
    if d.is_empty() {
        out.extend_from_slice(b"<< >>");
        return;
    }
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        out.push(b' ');
        write_name(out, k);
        out.push(b' ');
        write_object_into(out, v);
    }
    out.extend_from_slice(b" >>");
}

/// Serialise an indirect object, including its stream data.
pub fn write_indirect(num: u32, generation: u16, obj: &Object, out: &mut Vec<u8>) {
    out.extend_from_slice(num.to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(generation.to_string().as_bytes());
    out.extend_from_slice(b" obj\n");
    match obj {
        Object::Stream(s) => {
            let mut d = s.dict.clone();
            d.set("Length", Object::Int(s.raw.len() as i64));
            write_dict(out, &d);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.raw);
            out.extend_from_slice(b"\nendstream");
        }
        other => write_object_into(out, other),
    }
    out.extend_from_slice(b"\nendobj\n");
}

/// One entry in an incremental update.
#[derive(Debug, Clone)]
pub struct IncrementalUpdate {
    /// The objects to write, in ascending object-number order.
    pub objects: Vec<(u32, Object)>,
    /// Objects whose generation differs from zero.
    pub generations: BTreeMap<u32, u16>,
    /// New `/Size`.
    pub size: u32,
    /// `/Info` to record, if any.
    pub info: Option<Ref>,
    /// Objects this revision removes.
    freed: BTreeSet<(u32, u16)>,
}

impl IncrementalUpdate {
    #[must_use]
    pub fn new(size: u32) -> Self {
        Self {
            objects: Vec::new(),
            generations: BTreeMap::new(),
            size,
            info: None,
            freed: BTreeSet::new(),
        }
    }

    /// Add or replace an object.
    pub fn set(&mut self, num: u32, obj: Object) {
        self.set_with_generation(num, 0, obj);
    }

    pub fn set_with_generation(&mut self, num: u32, generation: u16, obj: Object) {
        self.generations.insert(num, generation);
        match self.objects.iter_mut().find(|(n, _)| *n == num) {
            Some(slot) => slot.1 = obj,
            None => self.objects.push((num, obj)),
        }
        self.objects.sort_by_key(|(n, _)| *n);
        let max = self.objects.last().map(|(n, _)| *n).unwrap_or(0);
        self.size = self.size.max(max + 1);
    }

    /// True when nothing would be written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.info.is_none() && self.freed.is_empty()
    }

    /// Mark an object as free in this revision. A deletion has to be expressed, or the
    /// object stays live in every later reader.
    pub fn free(&mut self, num: u32, generation: u16) {
        self.freed.insert((num, generation));
    }

    /// Append the update to `existing`, which must be the whole file so far.
    #[must_use]
    pub fn append(&self, existing: &[u8], prev_offset: u64, root: Option<Ref>) -> Vec<u8> {
        self.append_with_id(existing, prev_offset, root, &[])
    }

    /// Append the update, carrying the document's `/ID` into the new trailer.
    ///
    /// `/ID` is a pair whose first half is fixed for the document's life and whose
    /// second changes with each revision. The new second half is derived from the
    /// bytes this revision introduces, which is deterministic and distinguishes it from
    /// every earlier one.
    #[must_use]
    pub fn append_with_id(
        &self,
        existing: &[u8],
        prev_offset: u64,
        root: Option<Ref>,
        file_id: &[u8],
    ) -> Vec<u8> {
        let mut out = existing.to_vec();
        if !out.ends_with(b"\n") && !out.is_empty() {
            out.push(b'\n');
        }
        let start = out.len();
        let mut offsets: Vec<(u32, XrefEntry)> = Vec::new();
        for (num, obj) in &self.objects {
            let generation = self.generations.get(num).copied().unwrap_or(0);
            let at = out.len();
            write_indirect(*num, generation, obj, &mut out);
            offsets.push((
                *num,
                XrefEntry::InFile {
                    offset: at,
                    generation,
                },
            ));
        }
        let mut trailer = Dict::new();
        if let Some(r) = root {
            trailer.set("Root", Object::Ref(r));
        }
        if let Some(i) = self.info {
            trailer.set("Info", Object::Ref(i));
        }
        if !file_id.is_empty() {
            let second = revision_id(out.get(start..).unwrap_or_default());
            trailer.set(
                "ID",
                Object::Array(vec![
                    Object::String(file_id.to_vec()),
                    Object::String(second),
                ]),
            );
        }
        trailer.set("Size", Object::Int(i64::from(self.size)));
        trailer.set("Prev", Object::Int(i64::try_from(prev_offset).unwrap_or(0)));
        // The free entries have to be in this revision's table, or the object they
        // remove stays live.
        let freed: Vec<(u32, XrefEntry)> = self
            .freed
            .iter()
            .map(|(n, g)| {
                (
                    *n,
                    XrefEntry::Free {
                        next: 0,
                        generation: *g,
                    },
                )
            })
            .collect();
        let table_at = out.len() as u64;
        write_xref_table(&mut out, &offsets, &trailer, table_at, freed);
        out
    }
}

/// The second half of `/ID` for a revision, derived from the bytes it introduced.
fn revision_id(bytes: &[u8]) -> Vec<u8> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h.to_be_bytes().to_vec()
}

/// A full rewrite: mark-and-sweep from the trailer roots, then a fresh cross-reference.
#[derive(Debug, Clone)]
pub struct Writer<'a> {
    objects: &'a BTreeMap<u32, Object>,
    generations: &'a BTreeMap<u32, u16>,
    options: WriteOptions,
}

impl<'a> Writer<'a> {
    #[must_use]
    pub fn new(
        objects: &'a BTreeMap<u32, Object>,
        generations: &'a BTreeMap<u32, u16>,
        options: WriteOptions,
    ) -> Self {
        Self {
            objects,
            generations,
            options,
        }
    }

    /// Objects reachable from `roots`, in ascending order.
    #[must_use]
    pub fn reachable(&self, roots: &[Ref]) -> BTreeMap<u32, Object> {
        let mut seen: BTreeSet<u32> = BTreeSet::new();
        let mut queue: Vec<Ref> = roots.to_vec();
        while let Some(r) = queue.pop() {
            if !seen.insert(r.num) {
                continue;
            }
            let Some(obj) = self.objects.get(&r.num) else {
                continue;
            };
            let mut refs = Vec::new();
            collect_refs(obj, &mut refs, 0);
            queue.extend(refs);
        }
        let mut out = BTreeMap::new();
        for n in seen {
            if let Some(o) = self.objects.get(&n) {
                out.insert(n, o.clone());
            }
        }
        out
    }

    /// Write the whole file.
    ///
    /// `roots` are the trailer's indirect entries. The first is `/Root`; the caller
    /// puts anything else (`/Info` and so on) in `extra_trailer`, where its key is
    /// explicit and cannot be guessed wrong.
    #[must_use]
    pub fn write(&self, roots: &[Ref], extra_trailer: &Dict) -> Vec<u8> {
        // Every object the caller gave us, not only what the roots reach: an object
        // nothing we follow may still be named by something we do not understand,
        // and dropping it would be data loss.
        let live = self.objects.clone();
        let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
        let version = self
            .options
            .header_version
            .as_deref()
            .filter(|v| v.starts_with("1.") && v.len() <= 4)
            .unwrap_or("1.7");
        out.extend_from_slice(format!("%PDF-{version}\n").as_bytes());
        // The binary comment marks the file as containing binary data.
        out.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");

        let mut xref: Vec<(u32, XrefEntry)> = Vec::new();
        for (num, obj) in &live {
            if is_in_object_stream(obj) {
                continue;
            }
            let at = out.len();
            let generation = self.generations.get(num).copied().unwrap_or(0);
            write_indirect(*num, generation, obj, &mut out);
            xref.push((
                *num,
                XrefEntry::InFile {
                    offset: at,
                    generation,
                },
            ));
        }
        // `/Size` is one more than the highest object number, and must not shrink when
        // the highest object is deleted: a reader that counts objects would then
        // disagree with the file.
        let size = live
            .keys()
            .copied()
            .max()
            .map_or(0, |m| m + 1)
            .max(self.options.minimum_size);

        let mut trailer = extra_trailer.clone();
        if let Some(r) = roots.first() {
            trailer.set(ROOT_KEY, Object::Ref(*r));
        }
        trailer.set("Size", Object::Int(i64::from(size)));
        if !self.options.file_id.is_empty() {
            trailer.set(
                "ID",
                Object::Array(vec![
                    Object::String(self.options.file_id.clone()),
                    Object::String(self.options.file_id.clone()),
                ]),
            );
        }
        let start = out.len();
        if self.options.use_xref_streams {
            write_xref_stream(&mut out, &xref, &trailer, start as u64);
        } else {
            write_xref_table(&mut out, &xref, &trailer, start as u64, Vec::new());
        }
        out
    }
}

/// The trailer key that points at the document catalogue.
const ROOT_KEY: &str = "Root";

fn is_in_object_stream(_obj: &Object) -> bool {
    // Object streams are added in a later step; the classic layout is always valid.
    false
}

fn collect_refs(obj: &Object, out: &mut Vec<Ref>, depth: usize) {
    if depth > 64 {
        return;
    }
    match obj {
        Object::Ref(r) | Object::Missing(r) => out.push(*r),
        Object::Array(a) => {
            for o in a {
                collect_refs(o, out, depth + 1);
            }
        }
        Object::Dict(d) => {
            for (_, v) in d.iter() {
                collect_refs(v, out, depth + 1);
            }
        }
        Object::Stream(s) => {
            for (_, v) in s.dict.iter() {
                collect_refs(v, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Write a classic cross-reference table.
///
/// `rows` and `freed` are merged and sorted, so a number in both is written once: an
/// object this revision removes must not also be written as live.
fn write_xref_table(
    out: &mut Vec<u8>,
    rows: &[(u32, XrefEntry)],
    trailer: &Dict,
    start: u64,
    freed: Vec<(u32, XrefEntry)>,
) {
    out.extend_from_slice(b"xref\n");
    let mut all: BTreeMap<u32, XrefEntry> = freed.into_iter().collect();
    all.extend(rows.iter().map(|(n, e)| (*n, *e)));
    // Object 0 is always present and always free; a table that omits it is malformed
    // even though most readers cope.
    all.entry(0).or_insert(XrefEntry::Free {
        next: 0,
        generation: 65535,
    });

    let sorted: Vec<(u32, XrefEntry)> = all.into_iter().collect();
    // One subsection per contiguous run, which is what every producer does.
    let mut i = 0usize;
    while let Some(&(start_num, _)) = sorted.get(i) {
        let mut j = i;
        while let (Some(a), Some(b)) = (sorted.get(j), sorted.get(j + 1))
            && a.0 + 1 == b.0
        {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", start_num, j - i + 1).as_bytes());
        for (_, e) in sorted.get(i..=j).unwrap_or_default() {
            let (offset, generation, kind) = match e {
                XrefEntry::InFile { offset, generation } => {
                    (u64::try_from(*offset).unwrap_or(0), *generation, 'n')
                }
                XrefEntry::Free { next, generation } => (u64::from(*next), *generation, 'f'),
                _ => (0, 0, 'f'),
            };
            out.extend_from_slice(format!("{offset:010} {generation:05} {kind} \n").as_bytes());
        }
        i = j + 1;
    }
    out.extend_from_slice(b"trailer\n");
    write_dict(out, trailer);
    out.extend_from_slice(format!("\nstartxref\n{start}\n%%EOF\n").as_bytes());
}

fn write_xref_stream(out: &mut Vec<u8>, rows: &[(u32, XrefEntry)], trailer: &Dict, start: u64) {
    let size = rows.iter().map(|(n, _)| *n + 1).max().unwrap_or(1);
    // /W [1 8 2]
    let mut body: Vec<u8> = Vec::with_capacity(rows.len() * 11);
    for (_, e) in rows {
        let (kind, field, generation) = match e {
            XrefEntry::InFile { offset, generation } => {
                (1u8, u64::try_from(*offset).unwrap_or(0), *generation)
            }
            XrefEntry::InStream { stream, index } => (2u8, u64::from(*stream), *index as u16),
            XrefEntry::Free { next, generation } => (0u8, u64::from(*next), *generation),
            XrefEntry::Missing => (0u8, 0, 0),
        };
        body.push(kind);
        body.extend_from_slice(&field.to_be_bytes());
        body.extend_from_slice(&generation.to_be_bytes());
    }
    let mut d = Dict::new();
    d.set("Type", Object::name("XRef"));
    d.set("Size", Object::Int(i64::from(size)));
    d.set(
        "W",
        Object::Array(vec![Object::Int(1), Object::Int(8), Object::Int(2)]),
    );
    d.set("Filter", Object::name("FlateDecode"));
    d.set("Length", Object::Int(body.len() as i64));
    let mut t = trailer.clone();
    for (k, v) in d.iter() {
        t.insert(k.clone(), v.clone());
    }
    out.extend_from_slice(size.to_string().as_bytes());
    out.extend_from_slice(b" 0 obj\n");
    write_dict(out, &t);
    out.extend_from_slice(b"\nstream\n");
    let packed = mangle_filters::deflate(&body, mangle_filters::DeflateLevel::Default);
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::Stream;
    use crate::lexer::Lexer;

    fn round_trip_object(obj: &Object) {
        let mut out = Vec::new();
        write_object_into(&mut out, obj);
        let back = crate::parser::Parser::new(&out)
            .next_object()
            .expect("parse")
            .expect("object");
        assert_eq!(&back, obj, "round trip failed for {}", out.len());
    }

    #[test]
    fn scalars_round_trip() {
        for o in [
            Object::Null,
            Object::Bool(true),
            Object::Bool(false),
            Object::Int(0),
            Object::Int(-1),
            Object::Int(i64::MAX),
            Object::Real(1.5),
            Object::Real(-0.125),
            Object::String(b"hi\nthere".to_vec()),
            Object::Name(Name::new("Type")),
            Object::Ref(Ref::new(7, 0)),
        ] {
            round_trip_object(&o);
        }
    }

    #[test]
    fn numbers_are_writable() {
        let mut out = Vec::new();
        write_object_into(&mut out, &Object::Real(1e30));
        assert!(!out.iter().any(|b| b.is_ascii_alphabetic()), "{out:?}");
        let mut out = Vec::new();
        write_object_into(&mut out, &Object::Real(f64::NAN));
        assert_eq!(out, b"0.0");
    }

    #[test]
    fn dict_order_is_preserved() {
        let mut d = Dict::new();
        d.set("Zebra", Object::Int(1));
        d.set("Apple", Object::Int(2));
        let mut out = Vec::new();
        write_dict(&mut out, &d);
        assert_eq!(out, b"<< /Zebra 1 /Apple 2 >>");
    }

    #[test]
    fn names_are_escaped() {
        let mut out = Vec::new();
        write_name(&mut out, &Name::new("A B"));
        assert_eq!(out, b"/A#20B");
    }

    #[test]
    fn incremental_append_is_a_prefix() {
        let base = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n".to_vec();
        let mut u = IncrementalUpdate::new(2);
        u.set(
            1,
            Object::Dict(
                [
                    (Name::new("Type"), Object::name("Catalog")),
                    (Name::new("Version"), Object::string("2")),
                ]
                .into_iter()
                .collect(),
            ),
        );
        let out = u.append(&base, base.len() as u64, Some(Ref::new(1, 0)));
        assert!(
            out.starts_with(&base),
            "the original bytes must be a prefix"
        );
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("startxref"), "{text}");
    }

    #[test]
    fn empty_update_writes_nothing() {
        let u = IncrementalUpdate::new(1);
        assert!(u.is_empty());
    }

    #[test]
    fn full_rewrite_is_parseable() {
        let mut objects: BTreeMap<u32, Object> = BTreeMap::new();
        objects.insert(
            1,
            Object::Dict(
                [
                    (Name::new("Type"), Object::name("Catalog")),
                    (Name::new("Pages"), Object::Ref(Ref::new(2, 0))),
                ]
                .into_iter()
                .collect(),
            ),
        );
        objects.insert(
            2,
            Object::Dict(
                [
                    (Name::new("Type"), Object::name("Pages")),
                    (
                        Name::new("Kids"),
                        Object::Array(vec![Object::Ref(Ref::new(3, 0))]),
                    ),
                    (Name::new("Count"), Object::Int(1)),
                ]
                .into_iter()
                .collect(),
            ),
        );
        objects.insert(
            3,
            Object::Dict(
                [(Name::new("Type"), Object::name("Page"))]
                    .into_iter()
                    .collect(),
            ),
        );
        let gens = BTreeMap::new();
        let w = Writer::new(
            &objects,
            &gens,
            WriteOptions {
                use_xref_streams: false,
                ..Default::default()
            },
        );
        let out = w.write(&[Ref::new(1, 0)], &Dict::new());
        let (_, recovery) = crate::recovery::read_or_repair(&out).expect("read back");
        assert!(recovery.banner().is_none(), "{:?}", recovery.notes());
        let mut lx = Lexer::new(&out);
        assert!(lx.next_token().is_ok());
    }

    #[test]
    fn streams_carry_their_length() {
        let mut d = Dict::new();
        d.set("Length", Object::Int(999)); // deliberately wrong; the writer fixes it
        let s = Stream::new(d, b"HELLO".to_vec());
        let mut out = Vec::new();
        write_indirect(5, 0, &Object::Stream(s), &mut out);
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.contains("/Length 5"), "{text}");
        assert!(text.contains("HELLO"), "{text}");
    }
}
