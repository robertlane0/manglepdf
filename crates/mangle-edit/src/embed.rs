//! Embedding a subset for characters the run's own font does not have.
//!
//! The charter's ladder says a missing glyph is answered by embedding a subset, with a
//! notice naming the font and the reason (GOAL.md §4.4). This is the object half of that:
//! given a TrueType program and the characters needed, build the five indirect objects a
//! standards-compliant embedded CID font is — the `Type0` wrapper, the `CIDFontType2`
//! descendant, the `FontDescriptor`, the `ToUnicode`, and the `FontFile2` program — with
//! their references wired to object numbers the caller reserved. Installing them into a
//! page's resources and switching the run to them is the session's business, not here.
//!
//! ## The shape, and why it is this shape
//!
//! New text is always two-byte CIDs under `Identity-H`, even when every character is
//! Latin: a subset renumbers glyphs densely from zero, so single-byte codes would
//! collide with the run's old encoding the moment both appear. CIDs equal the new glyph
//! numbers, which makes `/CIDToGIDMap /Identity` exact rather than approximate, and the
//! `ToUnicode` maps each CID back to its character so copy, search and extraction of the
//! new text are exact.
//!
//! The subset tag is six uppercase letters from an FNV hash of the subset bytes:
//! deterministic (the same font and request build the same tag, which the determinism
//! gate requires) and distinct per subset (two different subsets never share a
//! `BaseFont`).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use mangle_font::{Program, glyph_for_unicode, subset_truetype};
use mangle_syntax::object::{Dict, Name, Object, Ref, Stream};

/// How many objects [`embed_truetype`] emits: the `Type0`, its descendant, the
/// descriptor, the `ToUnicode`, and the program, in that order.
pub const EMBEDDED_OBJECTS: u32 = 5;

/// Why a subset could not be embedded, named rather than approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// The subsetter refused the program or the request, with its reason.
    Subset(String),
    /// A kept character has no glyph in the subset built for it, which means the
    /// subsetter and this module disagree about the same font.
    NoCidFor(char),
    /// A table the descriptor is read from is missing or damaged.
    NoMetrics(&'static str),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subset(why) => write!(f, "{why}"),
            Self::NoCidFor(ch) => write!(
                f,
                "the subset has no glyph for {ch:?}, so there is no CID to write"
            ),
            Self::NoMetrics(what) => write!(f, "the font program has {what} to read"),
        }
    }
}

impl std::error::Error for EmbedError {}

/// An embedded subset, ready to install: five numbered objects and the CIDs they give.
#[derive(Debug, Clone)]
pub struct EmbeddedFont {
    /// The `Type0`'s number: the one a `Tf` names through the page's resources.
    pub type0: u32,
    /// All five objects, in number order starting at the reserved base.
    pub numbered: Vec<(u32, Object)>,
    /// Each requested character beside the CID it was given, in request order.
    pub cids: Vec<(char, u32)>,
}

/// Embed `program` for `chars` as five objects numbered from `first`.
///
/// `family` is the `BaseFont` stem the subset tag prefixes (`ABCDEF+LiberationSans`).
/// The program must be TrueType; the characters must be in the basic multilingual
/// plane and present in the program — all three are the subsetter's own refusals,
/// repeated here because they are the reasons an embedding does not happen.
pub fn embed_truetype(
    program: &[u8],
    chars: &[char],
    family: &str,
    first: u32,
) -> Result<EmbeddedFont, EmbedError> {
    let unicodes: Vec<u32> = chars.iter().map(|c| u32::from(*c)).collect();
    let subset = subset_truetype(program, &unicodes).map_err(EmbedError::Subset)?;
    // One CID per requested character, in a stable order: the subset numbers glyphs
    // densely from sorted old numbers, and Identity maps each CID to the same glyph.
    let mut seen = BTreeSet::new();
    let mut cids: Vec<(char, u32)> = Vec::new();
    for ch in chars {
        if !seen.insert(*ch) {
            continue;
        }
        let gid = glyph_for_unicode(&subset, u32::from(*ch))
            .map_err(EmbedError::Subset)?
            .ok_or(EmbedError::NoCidFor(*ch))?;
        cids.push((*ch, gid));
    }
    let tag = subset_tag(&subset);
    let base_name = format!("{tag}+{family}");
    let metrics = descriptor_metrics(&subset)?;
    let mut program_reader = Program::new(subset.clone());
    let upem = program_reader
        .units_per_em()
        .ok_or(EmbedError::NoMetrics("no units-per-em"))?;
    // Widths in thousandths, one per kept CID: the advance in font units scaled by the
    // em, exactly as the widths a `Tf` advances by.
    let mut widths: Vec<(u32, u16)> = Vec::new();
    for (_, cid) in &cids {
        let advance = program_reader.advance(*cid).unwrap_or(1000);
        let width = (u32::from(advance) * 1000 / u32::from(upem)).min(0xFFFF) as u16;
        widths.push((*cid, width));
    }
    widths.sort_unstable();

    let type0 = first;
    let descendant = first + 1;
    let descriptor = first + 2;
    let to_unicode = first + 3;
    let font_file = first + 4;

    let file_dict = {
        let mut d = Dict::new();
        d.insert(Name::new("Length1"), Object::Int(subset.len() as i64));
        d.insert(Name::new("Filter"), Object::name("FlateDecode"));
        d
    };
    let file_stream = Object::Stream(Stream::new(
        file_dict,
        mangle_filters::deflate(&subset, mangle_filters::DeflateLevel::Default),
    ));
    let descriptor_dict = {
        let mut d = Dict::new();
        d.insert(Name::new("Type"), Object::name("FontDescriptor"));
        d.insert(Name::new("FontName"), Object::name(&base_name));
        d.insert(Name::new("Flags"), Object::Int(32));
        let bbox = Object::Array(
            metrics
                .bbox
                .iter()
                .map(|v| Object::Int(i64::from(*v)))
                .collect(),
        );
        d.insert(Name::new("FontBBox"), bbox);
        d.insert(Name::new("ItalicAngle"), Object::Int(0));
        d.insert(Name::new("Ascent"), Object::Int(i64::from(metrics.ascent)));
        d.insert(
            Name::new("Descent"),
            Object::Int(i64::from(metrics.descent)),
        );
        d.insert(
            Name::new("CapHeight"),
            Object::Int(i64::from(metrics.cap_height)),
        );
        d.insert(Name::new("StemV"), Object::Int(80));
        d.insert(Name::new("FontFile2"), Object::Ref(Ref::new(font_file, 0)));
        d
    };
    let descendant_dict = {
        let mut d = Dict::new();
        d.insert(Name::new("Type"), Object::name("Font"));
        d.insert(Name::new("Subtype"), Object::name("CIDFontType2"));
        d.insert(Name::new("BaseFont"), Object::name(&base_name));
        let mut info = Dict::new();
        info.insert(Name::new("Registry"), Object::string("Adobe"));
        info.insert(Name::new("Ordering"), Object::string("Identity"));
        info.insert(Name::new("Supplement"), Object::Int(0));
        d.insert(Name::new("CIDSystemInfo"), Object::Dict(info));
        d.insert(Name::new("DW"), Object::Int(1000));
        let mut w = Vec::new();
        for (cid, width) in &widths {
            w.push(Object::Int(i64::from(*cid)));
            w.push(Object::Array(vec![Object::Int(i64::from(*width))]));
        }
        d.insert(Name::new("W"), Object::Array(w));
        d.insert(
            Name::new("FontDescriptor"),
            Object::Ref(Ref::new(descriptor, 0)),
        );
        d.insert(Name::new("CIDToGIDMap"), Object::name("Identity"));
        d
    };
    let to_unicode_stream = {
        let mut d = Dict::new();
        d.insert(Name::new("Type"), Object::name("Font"));
        d.insert(Name::new("Subtype"), Object::name("ToUnicode"));
        d
    };
    let to_unicode_stream = Object::Stream(Stream::new(
        to_unicode_stream,
        to_unicode_cmap(&cids).into_bytes(),
    ));
    let type0_dict = {
        let mut d = Dict::new();
        d.insert(Name::new("Type"), Object::name("Font"));
        d.insert(Name::new("Subtype"), Object::name("Type0"));
        d.insert(Name::new("BaseFont"), Object::name(&base_name));
        d.insert(Name::new("Encoding"), Object::name("Identity-H"));
        d.insert(
            Name::new("DescendantFonts"),
            Object::Array(vec![Object::Ref(Ref::new(descendant, 0))]),
        );
        d.insert(Name::new("ToUnicode"), Object::Ref(Ref::new(to_unicode, 0)));
        d
    };
    let numbered = vec![
        (type0, Object::Dict(type0_dict)),
        (descendant, Object::Dict(descendant_dict)),
        (descriptor, Object::Dict(descriptor_dict)),
        (to_unicode, to_unicode_stream),
        (font_file, file_stream),
    ];
    Ok(EmbeddedFont {
        type0,
        numbered,
        cids,
    })
}

/// The six-letter subset tag from an FNV hash of the subset bytes.
///
/// FNV rather than a dependency: the tag needs determinism, not security, and six
/// uppercase letters are what the subset-tag convention asks for.
fn subset_tag(subset: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in subset {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut tag = String::with_capacity(6);
    let mut value = hash;
    for _ in 0..6 {
        tag.push((b'A' + (value % 26) as u8) as char);
        value /= 26;
    }
    tag
}

/// The descriptor numbers read out of the subset's own tables.
struct DescriptorMetrics {
    bbox: [i16; 4],
    ascent: i16,
    descent: i16,
    cap_height: i16,
}

/// Ascent, descent, bounding box and cap height from `hhea`, `head` and `OS/2`.
///
/// The cap height falls back to zero when the program carries no `OS/2`: a conventional
/// fraction of the ascent would be an invented number wearing a measured one's clothes,
/// and the renderer draws from outlines rather than from this table.
fn descriptor_metrics(subset: &[u8]) -> Result<DescriptorMetrics, EmbedError> {
    let table = |tag: &[u8; 4]| -> Result<&[u8], EmbedError> {
        let num = u16at(subset, 4).ok_or(EmbedError::NoMetrics("no table directory"))?;
        if num as usize > 256 {
            return Err(EmbedError::NoMetrics("no table directory"));
        }
        for i in 0..num as usize {
            let at = 12 + i * 16;
            let found: [u8; 4] = subset
                .get(at..at + 4)
                .and_then(|s| <[u8; 4]>::try_from(s).ok())
                .ok_or(EmbedError::NoMetrics("a broken table directory"))?;
            if &found == tag {
                let offset =
                    u32at(subset, at + 8).ok_or(EmbedError::NoMetrics("a broken table"))? as usize;
                let length =
                    u32at(subset, at + 12).ok_or(EmbedError::NoMetrics("a broken table"))? as usize;
                return subset
                    .get(offset..offset.saturating_add(length))
                    .filter(|s| s.len() == length)
                    .ok_or(EmbedError::NoMetrics("a table past the file"));
            }
        }
        Err(EmbedError::NoMetrics("a missing table"))
    };
    let no = |what: &'static str| EmbedError::NoMetrics(what);
    let head = table(b"head").map_err(|_| no("no head table"))?;
    let hhea = table(b"hhea").map_err(|_| no("no hhea table"))?;
    if head.len() < 44 || hhea.len() < 8 {
        return Err(no("a short head or hhea table"));
    }
    let mut bbox = [0i16; 4];
    for (slot, at) in bbox.iter_mut().zip([36usize, 38, 40, 42]) {
        *slot = i16at(head, at).ok_or(no("a short head table"))?;
    }
    let ascent = i16at(hhea, 4).ok_or(no("a short hhea table"))?;
    let descent = i16at(hhea, 6).ok_or(no("a short hhea table"))?;
    // Cap height from OS/2 when the program carries one; zero otherwise, for the reason
    // above.
    let cap_height = table(b"OS/2")
        .ok()
        .and_then(|os2| i16at(os2, 88))
        .unwrap_or(0);
    Ok(DescriptorMetrics {
        bbox,
        ascent,
        descent,
        cap_height,
    })
}

/// A `ToUnicode` CMap naming each CID's character, in blocks of one hundred.
///
/// One hundred entries to a `beginbfchar` block is the format's own limit, so longer
/// requests are split rather than written as one block no reader accepts.
fn to_unicode_cmap(cids: &[(char, u32)]) -> String {
    let mut out = String::from(
        "%!PS-Adobe-3.0 Resource-CMap\n\
         %%DocumentNeededResources: procset CIDInit\n\
         %%IncludeResource: font Adobe-Identity-UCS\n\
         %%BeginResource: CMap (Adobe-Identity-UCS)\n\
         /CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <0000> <FFFF>\n\
         endcodespacerange\n",
    );
    // At least the header and trailer with no blocks for an empty request: a
    // `ToUnicode` with no entries is still a CMap, and a missing one would say the font
    // has no text.
    for block in cids.chunks(100) {
        if block.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{} beginbfchar", block.len());
        for (ch, cid) in block {
            let _ = writeln!(out, "<{cid:04X}> <{:04X}>", u32::from(*ch));
        }
        out.push_str("endbfchar\n");
    }
    out.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end\n\
         %%EndResource\n\
         %%EOF\n",
    );
    out
}

fn u16at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_be_bytes)
}

fn u32at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_be_bytes)
}

fn i16at(bytes: &[u8], at: usize) -> Option<i16> {
    u16at(bytes, at).map(|v| v as i16)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`; the panic-free rule is about what the
    // product does with a file, not about tests.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use mangle_syntax::object::{Dict, Object};

    use super::{EMBEDDED_OBJECTS, embed_truetype};

    fn liberation() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/LiberationSans-Regular.ttf");
        std::fs::read(&path).expect("the bundled face is checked in")
    }

    /// The embedded objects as dictionaries, by role: type0, descendant, descriptor.
    fn dicts(numbered: &[(u32, Object)]) -> (&Dict, &Dict, &Dict) {
        let dict = |n: usize| -> &Dict {
            numbered[n]
                .1
                .as_dict()
                .expect("font objects are dictionaries")
        };
        (dict(0), dict(1), dict(2))
    }

    #[test]
    fn five_objects_wired_to_the_reserved_numbers() {
        let data = liberation();
        let embedded =
            embed_truetype(&data, &['H', 'i'], "LiberationSans", 41).expect("two letters embed");
        assert_eq!(embedded.type0, 41);
        assert_eq!(embedded.numbered.len(), EMBEDDED_OBJECTS as usize);
        let numbers: Vec<u32> = embedded.numbered.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, vec![41, 42, 43, 44, 45]);
        let (type0, descendant, descriptor) = dicts(&embedded.numbered);
        let name = |o: &Object| o.as_name().map(|b| b.to_vec());
        assert_eq!(
            name(type0.get("Subtype").expect("subtype")),
            Some(b"Type0".to_vec())
        );
        assert_eq!(
            name(type0.get("Encoding").expect("encoding")),
            Some(b"Identity-H".to_vec())
        );
        // Every cross-reference lands on the reserved range: the descendant, the
        // `ToUnicode` and the program are found by number, not by search.
        let descendant_ref = type0
            .get("DescendantFonts")
            .and_then(|o| o.as_array())
            .and_then(|a| a.first())
            .and_then(|o| match o {
                Object::Ref(r) => Some(r.num),
                _ => None,
            });
        assert_eq!(descendant_ref, Some(42));
        let to_unicode = type0.get("ToUnicode").and_then(|o| match o {
            Object::Ref(r) => Some(r.num),
            _ => None,
        });
        assert_eq!(to_unicode, Some(44));
        assert_eq!(
            name(descendant.get("Subtype").expect("subtype")),
            Some(b"CIDFontType2".to_vec())
        );
        let file = descriptor.get("FontFile2").and_then(|o| match o {
            Object::Ref(r) => Some(r.num),
            _ => None,
        });
        assert_eq!(file, Some(45));
        // And the `ToUnicode` names both characters for the CIDs they were given.
        let to_unicode_bytes = match &embedded.numbered[3].1 {
            Object::Stream(s) => s.raw.clone(),
            other => panic!("a stream, not {other:?}"),
        };
        let text = String::from_utf8(to_unicode_bytes).expect("a CMap is text");
        for (ch, cid) in &embedded.cids {
            let line = format!("<{cid:04X}> <{:04X}>", u32::from(*ch));
            assert!(text.contains(&line), "{line} is in the ToUnicode");
        }
    }

    #[test]
    fn the_program_inflates_to_the_subset() {
        let data = liberation();
        let want = ['H', 'e', 'l', 'o'];
        let embedded = embed_truetype(&data, &want, "LiberationSans", 7).expect("embeds");
        let raw = match &embedded.numbered[4].1 {
            Object::Stream(s) => s.raw.clone(),
            other => panic!("a stream, not {other:?}"),
        };
        let inflated = mangle_filters::inflate(&raw, 1 << 20);
        assert!(inflated.complete, "the program decodes whole");
        let expected =
            mangle_font::subset_truetype(&data, &want.map(u32::from)).expect("the same subset");
        assert_eq!(inflated.data, expected);
    }

    #[test]
    fn widths_come_from_the_program() {
        let data = liberation();
        let embedded = embed_truetype(&data, &['H'], "LiberationSans", 7).expect("embeds");
        let (_, descendant, _) = dicts(&embedded.numbered);
        let w = descendant
            .get("W")
            .and_then(|o| o.as_array())
            .expect("widths");
        // One singleton run per CID: [cid [width]].
        assert_eq!(w.len(), 2);
        let cid = w[0].as_i64().expect("a CID");
        let width = w[1]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|o| o.as_i64())
            .expect("a width");
        let mut program = mangle_font::Program::new(data.clone());
        let upem = u32::from(program.units_per_em().expect("an em"));
        let gid = embedded.cids.iter().find(|(c, _)| *c == 'H').expect("H").1;
        assert_eq!(cid, i64::from(gid), "the width is H's");
        // The expectation is read out of the original program at H's *old* number: the
        // new CID lives in the subset's numbering, not the original's, and looking it
        // up there reads whatever glyph happens to sit at that number.
        let old = mangle_font::glyph_for_unicode(&data, u32::from('H'))
            .expect("readable")
            .expect("present");
        let advance = u32::from(program.advance(old).expect("an advance"));
        assert_eq!(width, i64::from(advance * 1000 / upem));
    }

    #[test]
    fn long_requests_split_the_tounicode() {
        let data = liberation();
        // Every printable ASCII character the face has, then Latin-1 past it: past one
        // hundred entries the CMap must split into a second block.
        let mut chars = Vec::new();
        for code in 0x20u32..0x100 {
            let Some(ch) = char::from_u32(code) else {
                continue;
            };
            if mangle_font::glyph_for_unicode(&data, code)
                .expect("readable")
                .is_some()
            {
                chars.push(ch);
            }
        }
        assert!(
            chars.len() > 100,
            "enough characters to split: {}",
            chars.len()
        );
        let embedded = embed_truetype(&data, &chars, "LiberationSans", 7).expect("embeds");
        let raw = match &embedded.numbered[3].1 {
            Object::Stream(s) => s.raw.clone(),
            other => panic!("a stream, not {other:?}"),
        };
        let text = String::from_utf8(raw).expect("a CMap is text");
        assert_eq!(text.matches("beginbfchar").count(), 2);
    }

    #[test]
    fn embedding_is_deterministic() {
        let data = liberation();
        let want = ['D', 'i', 's', 'c', 'o', 'v', 'e', 'r'];
        let first = embed_truetype(&data, &want, "LiberationSans", 7).expect("embeds");
        let second = embed_truetype(&data, &want, "LiberationSans", 9).expect("embeds");
        // The numbers follow their own base; everything else — the tag, the CIDs, the
        // program, the mapping — is identical, because nothing here reads a clock.
        let numbers: Vec<u32> = second.numbered.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, vec![9, 10, 11, 12, 13]);
        let name = |numbered: &[(u32, Object)]| -> Vec<u8> {
            dicts(numbered)
                .0
                .get("BaseFont")
                .and_then(|o| o.as_name())
                .expect("a BaseFont")
                .to_vec()
        };
        assert_eq!(name(&first.numbered), name(&second.numbered));
        let stream_text = |numbered: &[(u32, Object)], at: usize| -> Vec<u8> {
            match &numbered[at].1 {
                Object::Stream(s) => s.raw.clone(),
                other => panic!("a stream, not {other:?}"),
            }
        };
        assert_eq!(
            stream_text(&first.numbered, 3),
            stream_text(&second.numbered, 3),
            "the ToUnicode is identical"
        );
        assert_eq!(
            stream_text(&first.numbered, 4),
            stream_text(&second.numbered, 4),
            "the program is identical"
        );
        assert_eq!(first.cids, second.cids);
    }

    #[test]
    fn a_missing_glyph_stops_the_embedding() {
        let data = liberation();
        let err = embed_truetype(&data, &['H', '\u{378}'], "LiberationSans", 7)
            .expect_err("U+0378 is unassigned");
        assert!(err.to_string().contains("U+0378"), "{err}");
    }
}
