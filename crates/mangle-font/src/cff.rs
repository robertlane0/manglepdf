//! CFF outlines: the compact font format, and the Type 2 charstrings inside it.
//!
//! A CFF font is a small table format wrapped around a stack machine. The wrapper is a
//! header, four INDEXes, and a handful of DICTs naming where the glyph programs are; the
//! programs themselves are Type 2 charstrings, which are not outlines but a *way of
//! computing* one. A `CFF ` table therefore cannot be read by walking points the way a
//! `glyf` table can — it has to be executed.
//!
//! ## What is read, and what is left out
//!
//! Read: the header, the INDEXes, the DICTs, and every charstring operator that draws or
//! hints. That is the whole of the geometry.
//!
//! Left out, on purpose:
//!
//! * **The `Name` and `String` INDEXes and every glyph name.** Nothing below needs to know
//!   that glyph 4 is `H`; a charstring is addressed by number.
//! * **Every Private DICT key except `Subrs`, `defaultWidthX` and `nominalWidthX`.** The
//!   rest — `BlueValues`, `StdHW`, `ExpansionFactor` — describe how to *hint* stems to a
//!   rasterizer's pixel grid. This renderer computes exact analytic coverage and has no
//!   grid to snap to, so a hint has nothing to act on.
//! * **`seac`.** `endchar` with four or five operands builds an accented character out of
//!   two others *by name*, which needs the charset and the standard encoding. That is
//!   reported rather than skipped: an accented character drawn as an unaccented one is a
//!   wrong answer, not a missing one.
//! * **The variation store's region scalars.** `vsindex` and `blend` *are* executed, and
//!   a blend with no scalars to add is its default values — which is the default instance,
//!   and is what a PDF asks for. A font at a named location is not representable here and
//!   is not claimed.
//!
//! ## CID fonts
//!
//! A CID-keyed font gives every glyph its own Private DICT through `/FDArray` and
//! `/FDSelect`, so which subroutines a glyph may call is a property of the glyph and not
//! of the font. That is read: both `FDSelect` formats, the Font DICT array, and each FD's
//! `Subrs` and widths.
//!
//! What is *not* claimed is the other half. A CID font maps character identifiers to glyphs
//! through its `charset`, and only the identity mapping is read. A CID font whose charset
//! says something else is **refused with a reason** rather than drawn, because treating its
//! identifiers as glyph numbers produces the wrong shape for every character and nothing
//! downstream can tell.
//!
//! ## Bounds
//!
//! [`MAX_STACK`] and [`MAX_DEPTH`] are the format's limits, and passing either is damage
//! rather than a long program. Both report a reason instead of growing the bound.

use ttf_parser::{Face, Tag};

use crate::outline::{Outline, Segment};

/// The most values a Type 2 charstring may hold on its operand stack.
///
/// The specification's limit, not a convenient round number. A charstring that goes past
/// it is malformed or hostile, and raising the bound to accommodate one would make the
/// limit worth nothing.
pub const MAX_STACK: usize = 48;

/// The most subroutine calls a charstring may nest inside one another.
pub const MAX_DEPTH: usize = 10;

/// The most operands one DICT key may be preceded by.
///
/// A `FontBBox` has four, an `ROS` has three, and the longest `deltaArray` the
/// specification allows has fourteen. Every one of them fits with room to spare.
const MAX_DICT_OPERANDS: usize = 512;

/// The most nibbles one binary-coded decimal may carry.
///
/// A DICT real number is a decimal *expression*, and the expression is what bounds it: a
/// file can write a thousand digits, and parsing them all before noticing would be a way to
/// make a font program expensive.
const MAX_REAL_NIBBLES: usize = 64;

/// The em a CFF font's own grid has to the em when its `FontMatrix` is the usual one.
///
/// This is what a bare CFF program reports, since it carries no `head` table.
const DEFAULT_UNITS_PER_EM: u16 = 1000;

/// The `FontMatrix` a CFF font has when it does not say: a thousandth of an em.
const DEFAULT_FONT_MATRIX: [f32; 6] = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];

// ── INDEX ─────────────────────────────────────────────────────────────────────────

/// The empty INDEX, shared. `Vec::new` is a constant, so this needs no initialisation.
static NO_OBJECTS: Index<'static> = Index {
    data: &[],
    start: 0,
    base: 0,
    offsets: Vec::new(),
    count_size: 2,
};

/// A CFF INDEX: an array of variable-length byte ranges, and nothing else.
///
/// `count` is a `u16` in CFF 1.0 and a `u32` in CFF 2.0; `count_size` says which, and it
/// is the one thing that differs between the versions. After the count come a one-byte
/// offset width and `count + 1` offsets, and every offset is measured from the byte
/// *before* the data — an offset of one is the first byte of the data, so the base the
/// offsets are added to is one back from where the array ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Index<'a> {
    data: &'a [u8],
    /// Where this INDEX begins, which an empty INDEX does not record.
    start: usize,
    /// The byte the offsets are measured from: the one before the object data.
    base: usize,
    offsets: Vec<usize>,
    /// How wide the count was, which is the whole of an empty INDEX's length.
    count_size: usize,
}

impl<'a> Index<'a> {
    /// An INDEX with nothing in it, which is what a font with no subroutines has.
    #[must_use]
    pub fn empty() -> Self {
        NO_OBJECTS.clone()
    }

    /// The empty INDEX as something with no lifetime attached to it.
    ///
    /// A borrowed value is the right answer for "this font has no local subroutines" and a
    /// freshly built one would be a temporary that cannot be returned. One shared value
    /// with no lifetimes in it at all is the thing that can.
    fn none() -> &'static Index<'static> {
        &NO_OBJECTS
    }

    /// The INDEX beginning at `at`, or `None` if the bytes there are not one.
    ///
    /// A file can put an offset anywhere, so every step is checked. A four-byte `count` of
    /// four billion must cost the four bytes its header claims rather than the four
    /// gigabytes it would like, which is what bounding the array by the file before
    /// allocating for it is for.
    #[must_use]
    pub fn parse(data: &'a [u8], at: usize, count_size: usize) -> Option<Self> {
        let count = match count_size {
            2 => usize::from(be16(data, at)?),
            4 => usize::try_from(be32(data, at)?).ok()?,
            // CFF 1 writes a 16-bit count and CFF 2 a 32-bit one. A file wanting a third
            // width is not one of them.
            _ => return None,
        };
        if count == 0 {
            return Some(Self {
                count_size,
                ..Self::empty()
            });
        }
        let off_size = usize::from(*data.get(at.checked_add(count_size)?)?);
        // The specification names four offset widths. A fifth is a file being wrong rather
        // than the format being different.
        if !(1..=4).contains(&off_size) {
            return None;
        }
        let array = at.checked_add(count_size + 1)?;
        let offsets_end = array.checked_add((count + 1).checked_mul(off_size)?)?;
        if offsets_end > data.len() {
            return None;
        }
        let mut offsets = Vec::with_capacity(count + 1);
        for i in 0usize..=count {
            let start = array.checked_add(i.checked_mul(off_size)?)?;
            let mut value = 0usize;
            for k in 0..off_size {
                value = (value << 8) | usize::from(*data.get(start.checked_add(k)?)?);
            }
            offsets.push(value);
        }
        // The first offset is one by definition, and a decreasing array would give an
        // object a negative length. Either way the file is wrong.
        if offsets.first().copied() != Some(1) {
            return None;
        }
        if offsets.windows(2).any(|w| match w {
            [a, b] => a > b,
            _ => true,
        }) {
            return None;
        }
        let base = offsets_end.checked_sub(1)?;
        Some(Self {
            data,
            start: at,
            base,
            offsets,
            count_size,
        })
    }

    /// How many objects this INDEX holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Is there nothing in here?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One object's bytes, or `None` if there is no object at that index.
    ///
    /// An object outside the INDEX is `None` rather than a clamp, because `callsubr` adds a
    /// bias to its operand and a bias applied wrongly lands on a *different, in-range*
    /// index and runs the wrong program.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a [u8]> {
        let start = self.offsets.get(index)?;
        let end = self.offsets.get(index + 1)?;
        let from = self.base.checked_add(*start)?;
        let to = self.base.checked_add(*end)?;
        self.data.get(from..to)
    }

    /// How many bytes this INDEX occupies, so that the next one can be found.
    ///
    /// CFF 1 puts its four INDEXes one after another with nothing between them, so the
    /// third is found by adding the lengths of the first two. An object's last byte is the
    /// one the final offset names, and that byte *is* the last byte of the INDEX.
    fn len_of(&self) -> usize {
        if self.offsets.is_empty() {
            // An empty INDEX is a count of zero and nothing else, so its length is exactly
            // as wide as that count: two bytes in CFF 1 and four in CFF 2.
            return self.count_size;
        }
        let end = self
            .base
            .saturating_add(self.offsets.last().copied().unwrap_or(1));
        end.saturating_sub(self.start)
    }
}

fn be16(data: &[u8], at: usize) -> Option<u16> {
    let mut out = [0u8; 2];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = *data.get(at.checked_add(i)?)?;
    }
    Some(u16::from_be_bytes(out))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let mut out = [0u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = *data.get(at.checked_add(i)?)?;
    }
    Some(u32::from_be_bytes(out))
}

fn be8(data: &[u8], at: usize) -> Result<u8, String> {
    data.get(at).copied().ok_or_else(|| why("it is cut short"))
}

/// A failure reason from a static message.
fn why(message: &str) -> String {
    message.to_owned()
}

// ── DICT ──────────────────────────────────────────────────────────────────────────

/// A DICT: a run of operands, each followed by the operator that names what they mean.
///
/// A key is one byte, or two when the first is the escape `12`, which is why it fits in a
/// `u16` with the escape written as `0x0c00`. The values are kept exactly as written — a
/// flat list of numbers — because how many a key takes is a property of the key and of
/// which DICT it was found in, and guessing here would be guessing twice.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict {
    entries: Vec<(u16, Vec<f32>)>,
}

impl Dict {
    /// The DICT in `data`, or `None` if the bytes are not one.
    ///
    /// A malformed DICT is refused rather than half-read: an offset read out of a DICT that
    /// desynchronised halfway through points into the middle of nothing.
    #[must_use]
    pub fn parse(data: &[u8]) -> Option<Self> {
        let mut entries: Vec<(u16, Vec<f32>)> = Vec::new();
        let mut operands: Vec<f32> = Vec::new();
        let mut at = 0usize;
        while at < data.len() {
            if operands.len() > MAX_DICT_OPERANDS {
                // A DICT that never stops pushing operands is not describing a font.
                return None;
            }
            let b0 = *data.get(at)?;
            at += 1;
            let key = match b0 {
                // The three operand forms a DICT has and a charstring does not.
                28 => {
                    operands.push(read_short_int(data, &mut at)?);
                    continue;
                }
                29 => {
                    operands.push(read_long_int(data, &mut at)?);
                    continue;
                }
                30 => {
                    operands.push(read_real(data, &mut at)?);
                    continue;
                }
                32..=246 => {
                    operands.push(f32::from(b0) - 139.0);
                    continue;
                }
                247..=250 => {
                    let b1 = *data.get(at)?;
                    at += 1;
                    operands.push(f32::from(b0 - 247) * 256.0 + f32::from(b1) + 108.0);
                    continue;
                }
                251..=254 => {
                    let b1 = *data.get(at)?;
                    at += 1;
                    operands.push(-(f32::from(b0 - 251) * 256.0 + f32::from(b1) + 108.0));
                    continue;
                }
                12 => {
                    let b1 = *data.get(at)?;
                    at += 1;
                    0x0c00 | u16::from(b1)
                }
                // 0..=27 are all operators. 31 is not an operand encoding and 255 is
                // reserved in a DICT, so both are a file being wrong.
                31 | 255 => return None,
                other => u16::from(other),
            };
            entries.push((key, std::mem::take(&mut operands)));
        }
        Some(Self { entries })
    }

    /// Every operand the named key was preceded by, in the order written.
    #[must_use]
    pub fn values(&self, key: u16) -> &[f32] {
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(&[][..], |(_, v)| v.as_slice())
    }

    /// The first operand the named key was preceded by.
    #[must_use]
    pub fn number(&self, key: u16) -> Option<f32> {
        self.values(key).first().copied()
    }

    /// The first two operands the named key was preceded by.
    #[must_use]
    pub fn pair(&self, key: u16) -> Option<(f32, f32)> {
        match self.values(key) {
            [a, b, ..] => Some((*a, *b)),
            _ => None,
        }
    }

    /// Is the key present at all? A key with no operands and a key that is absent are
    /// different questions, and `charset` in particular is only a real offset if it has
    /// one.
    #[must_use]
    pub fn has(&self, key: u16) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }
}

/// A two-byte signed operand, `b0` = 28.
fn read_short_int(data: &[u8], at: &mut usize) -> Option<f32> {
    let raw = be16(data, *at)?;
    *at = at.checked_add(2)?;
    Some(f32::from(i16::from_be_bytes(raw.to_be_bytes())))
}

/// A four-byte signed operand, `b0` = 29. A DICT only; a charstring cannot carry one.
fn read_long_int(data: &[u8], at: &mut usize) -> Option<f32> {
    let raw = be32(data, *at)?;
    *at = at.checked_add(4)?;
    Some(i32::from_be_bytes(raw.to_be_bytes()) as f32)
}

/// A binary-coded decimal, `b0` = 30: a decimal expression in four-bit nibbles.
///
/// `0`–`9` are digits, `a` is a point, `b` and `c` are `E` and `E-`, `e` is a minus sign,
/// and `f` ends the number. It is assembled as text and parsed once, which is the only
/// honest way to read something that is not a number but a *description* of one.
fn read_real(data: &[u8], at: &mut usize) -> Option<f32> {
    let mut text = String::with_capacity(MAX_REAL_NIBBLES);
    loop {
        let byte = *data.get(*at)?;
        *at = at.checked_add(1)?;
        for nibble in [byte >> 4, byte & 0x0f] {
            match nibble {
                0..=9 => text.push(char::from(b'0' + nibble)),
                0x0a => text.push('.'),
                0x0b => text.push('E'),
                0x0c => text.push_str("E-"),
                // Reserved by the specification. Reading past it would take whatever
                // follows as part of this number.
                0x0d => return None,
                0x0e => text.push('-'),
                _ => {
                    // A terminator in the high nibble ends the number, and its padding
                    // nibble has already been consumed along with the byte.
                    return text.parse::<f32>().ok().filter(|v| v.is_finite());
                }
            }
            if text.len() >= MAX_REAL_NIBBLES {
                return None;
            }
        }
    }
}

// ── DICT keys ─────────────────────────────────────────────────────────────────────

/// `charset`: the glyph-to-name table, which for a CID font is the identifier map.
const KEY_CHARSET: u16 = 15;
/// `CharStrings`: where the glyph programs are.
const KEY_CHARSTRINGS: u16 = 17;
/// `Private`: the size and offset of a name-keyed font's Private DICT.
const KEY_PRIVATE: u16 = 18;
/// `Subrs`: where a Private DICT's subroutines are, measured from the Private DICT.
const KEY_SUBRS: u16 = 19;
/// `defaultWidthX`: the width of a glyph whose charstring did not name one.
const KEY_DEFAULT_WIDTH: u16 = 20;
/// `nominalWidthX`: the width of a glyph with no stems at all.
const KEY_NOMINAL_WIDTH: u16 = 21;
/// `FontMatrix`: glyph units into ems.
const KEY_FONT_MATRIX: u16 = 0x0c07;
/// `ROS`: registry, ordering, supplement. Its presence is what makes a font CID-keyed.
const KEY_ROS: u16 = 0x0c1e;
/// `FDArray`: the Font DICT array of a CID-keyed font.
const KEY_FDARRAY: u16 = 0x0c24;
/// `FDSelect`: which Font DICT each glyph uses.
const KEY_FDSELECT: u16 = 0x0c25;
/// `VarStore`: the item variation store a CFF2 font's blends are measured against.
const KEY_VARSTORE: u16 = 24;
/// `CharstringType`: 1 for the Type 1 dialect of charstrings, 2 for Type 2. The format's
/// default is 2, and a font that does not say has Type 2 charstrings.
const KEY_CHARSTRING_TYPE: u16 = 0x0c06;

// ── the font ──────────────────────────────────────────────────────────────────────

/// One Private DICT's two numbers a charstring walk needs, plus its subroutines.
#[derive(Debug, Clone, PartialEq)]
struct Private<'a> {
    nominal_width_x: f32,
    default_width_x: f32,
    subrs: Index<'a>,
}

/// Where a glyph's private data comes from.
#[derive(Debug, Clone, PartialEq)]
enum Privates<'a> {
    /// A name-keyed font: one Private DICT behind every glyph.
    One(Private<'a>),
    /// A CID-keyed font: one per Font DICT, and `/FDSelect` says which.
    PerGlyph {
        fds: Vec<Private<'a>>,
        select: Vec<u16>,
    },
}

impl<'a> Privates<'a> {
    fn for_glyph(&self, glyph: u32) -> Option<&Private<'a>> {
        match self {
            Self::One(one) => Some(one),
            Self::PerGlyph { fds, select } => {
                let fd = select.get(usize::try_from(glyph).ok()?)?;
                fds.get(usize::from(*fd))
            }
        }
    }
}

/// A CFF font, as much of one as a charstring walk needs.
///
/// The four things every walk asks for are fields: the glyph programs, the global
/// subroutines, the widths, and the matrix that turns glyph units into ems. The rest is
/// private because it exists only to answer those four.
#[derive(Debug, Clone, PartialEq)]
pub struct Cff<'a> {
    /// One charstring per glyph, in glyph order.
    pub charstrings: Index<'a>,
    /// Subroutines any glyph in the font may call.
    pub global_subrs: Index<'a>,
    /// The `nominalWidthX` every glyph of a name-keyed font shares. A CID font's glyphs
    /// have their own; [`Self::widths`] is the answer to the question in general.
    pub nominal_width_x: f32,
    /// The `defaultWidthX` every glyph of a name-keyed font shares.
    pub default_width_x: f32,
    /// `[a b c d e f]`, mapping a glyph's own units into ems. A thousandth of an em by
    /// default, which is to say a CFF font's grid is a thousand units to the em unless it
    /// says otherwise.
    pub font_matrix: [f32; 6],
    privates: Privates<'a>,
    /// How many variation regions each item-variation record has. `blend` cannot know how
    /// many operands it is eating without it, so without a store a blend is damage.
    regions: Option<Vec<u32>>,
    cff2: bool,
    cid_keyed: bool,
}

impl<'a> Cff<'a> {
    /// The CFF table in `data`, or the reason there is not one.
    ///
    /// `font` selects which font of the table to read. A bare CFF in a PDF, and the `CFF `
    /// table of an OpenType wrapper, each hold one; a CFF file may hold several and the
    /// number is the index of the wanted one.
    pub fn parse(data: &'a [u8], font: u32) -> Result<Self, String> {
        let major = *data.first().ok_or("there are no bytes at all")?;
        let cff2 = match major {
            1 => false,
            2 => true,
            other => {
                return Err(format!(
                    "its first byte is {other}, which is not a CFF major version"
                ));
            }
        };
        let header_size = usize::from(*data.get(2).ok_or("its header is cut short")?);
        // The header size is where the Name INDEX begins in CFF 1 and where the Top DICT
        // begins in CFF 2, so a font claiming a header longer than it is has a header that
        // runs into its own tables.
        if header_size < if cff2 { 5 } else { 4 } || header_size > data.len() {
            return Err(format!("it claims a {header_size}-byte header"));
        }
        let count_size = if cff2 { 4 } else { 2 };

        let (top, global_subrs) = if cff2 {
            let top_len = usize::from(be16(data, 3).ok_or("its header is cut short")?);
            let end = header_size
                .checked_add(top_len)
                .ok_or("its Top DICT length overflows")?;
            let bytes = data
                .get(header_size..end)
                .ok_or("its Top DICT is cut short")?;
            let subrs = Index::parse(data, end, count_size)
                .ok_or("its global subroutines are not an INDEX")?;
            (
                Dict::parse(bytes).ok_or("its Top DICT is malformed")?,
                subrs,
            )
        } else {
            // CFF 1 states no sizes: the four INDEXes follow one another, so each is found
            // by adding the length of the ones before it.
            let top_bytes = top_dict_index(data, header_size, font)?;
            let top = Dict::parse(&top_bytes).ok_or("its Top DICT is malformed")?;
            let subrs = global_subr_index(data, header_size)?;
            (top, subrs)
        };

        let at = top
            .number(KEY_CHARSTRINGS)
            .and_then(offset)
            .ok_or("its Top DICT does not say where the CharStrings are")?;
        let charstrings = Index::parse(data, at, count_size)
            .ok_or("the CharStrings INDEX it points at is malformed")?;
        if charstrings.is_empty() {
            return Err("it has no glyphs at all".into());
        }

        // The charstring dialect decides which interpreter the font needs. Type 1
        // charstrings are a different language — `hsbw` rather than a width operand,
        // `closepath`, `callothersubr` — and running them as Type 2 draws a wrong shape
        // rather than no shape, so the font is refused here, where the reason can say so,
        // rather than further in, where it cannot.
        if top
            .number(KEY_CHARSTRING_TYPE)
            .is_some_and(|t| (t - 2.0).abs() > 0.5)
        {
            return Err(
                "its Top DICT says its charstrings are Type 1, which this does not \
                 interpret: Type 1 charstrings are a different language, and running them \
                 as Type 2 would draw the wrong shape for every glyph"
                    .into(),
            );
        }

        let font_matrix = font_matrix(&top);
        let cid_keyed = top.has(KEY_ROS);
        if cid_keyed && !identity_charset(data, &top, charstrings.len()) {
            return Err(
                "it is CID-keyed and its charset is not the identity mapping, so its \
                 character identifiers are not its glyph numbers, and drawing them as if \
                 they were would draw the wrong shape for every character"
                    .into(),
            );
        }
        let privates = if cid_keyed || top.number(KEY_FDARRAY).is_some() {
            Privates::PerGlyph {
                fds: fd_arrays(data, &top, count_size)?,
                select: fd_select(data, &top, charstrings.len())?,
            }
        } else {
            Privates::One(private(data, &top, 0)?)
        };
        // Only a name-keyed font has one set of widths to call the font's own; a CID
        // font's are per glyph and asked for with [`Self::widths`].
        let (nominal_width_x, default_width_x) = match &privates {
            Privates::One(one) => (one.nominal_width_x, one.default_width_x),
            Privates::PerGlyph { fds, .. } => fds
                .first()
                .map_or((0.0, 0.0), |f| (f.nominal_width_x, f.default_width_x)),
        };
        let regions = if cff2 { regions(data, &top) } else { None };

        Ok(Self {
            charstrings,
            global_subrs,
            nominal_width_x,
            default_width_x,
            font_matrix,
            privates,
            regions,
            cff2,
            cid_keyed,
        })
    }

    /// How many glyphs the font has.
    #[must_use]
    pub fn num_glyphs(&self) -> usize {
        self.charstrings.len()
    }

    /// Is this a CID-keyed font, whose character identifiers are not glyph names?
    #[must_use]
    pub fn is_cid_keyed(&self) -> bool {
        self.cid_keyed
    }

    /// Is this CFF 2, whose charstrings may use `vsindex` and `blend`?
    #[must_use]
    pub fn is_cff2(&self) -> bool {
        self.cff2
    }

    /// The local subroutines one glyph's charstring may call.
    #[must_use]
    pub fn local_subrs(&self, glyph: u32) -> &Index<'a> {
        match self.privates.for_glyph(glyph) {
            Some(private) => &private.subrs,
            None => Index::none(),
        }
    }

    /// `(nominalWidthX, defaultWidthX)` for one glyph, which a CID font varies per glyph.
    #[must_use]
    pub fn widths(&self, glyph: u32) -> (f32, f32) {
        match self.privates.for_glyph(glyph) {
            Some(p) => (p.nominal_width_x, p.default_width_x),
            None => (self.nominal_width_x, self.default_width_x),
        }
    }

    /// One glyph's outline, in ems, or the reason there is not one.
    pub fn outline(&self, glyph: u32) -> Result<Outline, String> {
        let code = self.charstring(glyph)?;
        let walker = Walker::new(self, self.local_subrs(glyph).clone(), glyph);
        Ok(walker.walk(code)?.outline)
    }

    /// How wide one glyph is, in thousandths of an em, or the reason there is not one.
    ///
    /// A charstring's own width is in glyph units, which the default `FontMatrix` makes
    /// thousandths of an em. A font that scales its glyphs has that scale applied, so the
    /// answer means the same thing here as the `/Widths` array it will be written into.
    pub fn advance(&self, glyph: u32) -> Result<u16, String> {
        let code = self.charstring(glyph)?;
        let width = Walker::new(self, self.local_subrs(glyph).clone(), glyph)
            .walk(code)?
            .width
            .ok_or("that glyph's charstring names no width")?;
        // Glyph units, to ems, to thousandths of an em.
        let scale = f64::from(self.font_matrix.first().copied().unwrap_or(0.001));
        let mils = f64::from(width) * 1000.0 * scale;
        Ok(mils.clamp(0.0, f64::from(u16::MAX)).round() as u16)
    }

    /// The bias to add to a `callsubr` or `callgsubr` operand, given how many subroutines
    /// there are.
    ///
    /// Three ranges, and the boundaries are the whole difficulty: 107 below 1240
    /// subroutines, 1131 below 33 900, and 32768 above. The wrong answer at a boundary is
    /// not a wrong number — it is a *different subroutine*, drawing a plausible shape for
    /// the wrong reason.
    #[must_use]
    pub fn subr_bias(count: usize) -> usize {
        if count < 1240 {
            107
        } else if count < 33900 {
            1131
        } else {
            32768
        }
    }

    /// One glyph's charstring.
    fn charstring(&self, glyph: u32) -> Result<&'a [u8], String> {
        let count = self.num_glyphs();
        let index = usize::try_from(glyph).unwrap_or(usize::MAX);
        if index >= count {
            return Err(format!(
                "glyph {glyph} is not one of its {count}, the highest being {}",
                count.saturating_sub(1)
            ));
        }
        self.charstrings
            .get(index)
            .ok_or_else(|| why("its CharStrings INDEX has a hole where that glyph should be"))
    }
}

/// The bytes of a CFF 1 font's Top DICT.
///
/// The Name INDEX comes first and the Top DICT INDEX second, with neither stating a size,
/// so the second is found by adding the length of the first.
fn top_dict_index(data: &[u8], header_size: usize, font: u32) -> Result<Vec<u8>, String> {
    let name = Index::parse(data, header_size, 2).ok_or("its Name INDEX is malformed")?;
    let wanted = usize::try_from(font).unwrap_or(0);
    let tops = Index::parse(
        data,
        header_size
            .checked_add(name.len_of())
            .ok_or("its header length overflows")?,
        2,
    )
    .ok_or("its Top DICT INDEX is malformed")?;
    tops.get(wanted)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "it has no Top DICT at that index".to_string())
}

/// The bytes of a CFF 1 font's global subroutines, the fourth and last INDEX.
fn global_subr_index(data: &[u8], header_size: usize) -> Result<Index<'_>, String> {
    let mut at = header_size;
    for what in ["Name INDEX", "Top DICT INDEX", "String INDEX"] {
        let index = Index::parse(data, at, 2).ok_or_else(|| format!("its {what} is malformed"))?;
        at = at
            .checked_add(index.len_of())
            .ok_or("its INDEX length overflows")?;
    }
    Index::parse(data, at, 2).ok_or_else(|| "its global subroutines are not an INDEX".to_string())
}

/// An operand that is an offset: a whole number of bytes into the table.
fn offset(value: f32) -> Option<usize> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    usize::try_from(value as u64).ok()
}

/// The font matrix, or the default one.
///
/// A matrix that scales a glyph to nothing, or to something that is not a number, would
/// make every outline a point or an infinity. The default is the font's own grid.
fn font_matrix(top: &Dict) -> [f32; 6] {
    let mut matrix = DEFAULT_FONT_MATRIX;
    if let [a, b, c, d, e, f] = top.values(KEY_FONT_MATRIX) {
        matrix = [*a, *b, *c, *d, *e, *f];
    }
    if !matrix.iter().all(|v| v.is_finite()) || matrix[0].abs() < f32::EPSILON {
        return DEFAULT_FONT_MATRIX;
    }
    matrix
}

/// A Private DICT named by the `(size, offset)` pair a DICT key was written with.
///
/// `base` is where that pair was found, because the offset in it is measured from there:
/// the start of the table for a Font DICT's `Private`, and the same for a Top DICT's.
fn private<'a>(data: &'a [u8], dict: &Dict, base: usize) -> Result<Private<'a>, String> {
    let Some((size, at)) = dict.pair(KEY_PRIVATE) else {
        return Ok(Private {
            nominal_width_x: 0.0,
            default_width_x: 0.0,
            subrs: Index::empty(),
        });
    };
    let (size, at) = (offset(size).unwrap_or(0), offset(at).unwrap_or(0));
    let start = base.checked_add(at).unwrap_or(base);
    let end = start.checked_add(size).unwrap_or(start);
    let bytes = data
        .get(start..end)
        .ok_or("the Private DICT it points at is cut short")?;
    let dict = Dict::parse(bytes).ok_or("its Private DICT is malformed")?;
    // `/Subrs` is measured from the start of the Private DICT and not from the start of
    // the table, which is the one offset in this format that is not where it looks.
    let subrs = match dict.number(KEY_SUBRS).and_then(offset) {
        Some(relative) => Index::parse(data, start.saturating_add(relative), 2)
            .ok_or("the local subroutines it points at are malformed")?,
        None => Index::empty(),
    };
    Ok(Private {
        nominal_width_x: dict.number(KEY_NOMINAL_WIDTH).unwrap_or(0.0),
        default_width_x: dict.number(KEY_DEFAULT_WIDTH).unwrap_or(0.0),
        subrs,
    })
}

/// The Font DICT array of a CID-keyed font: one Private DICT per FD.
fn fd_arrays<'a>(
    data: &'a [u8],
    top: &Dict,
    count_size: usize,
) -> Result<Vec<Private<'a>>, String> {
    let at = top
        .number(KEY_FDARRAY)
        .and_then(offset)
        .ok_or("it is CID-keyed but names no FDArray")?;
    let array = Index::parse(data, at, count_size).ok_or("its FDArray is not an INDEX")?;
    let mut fds = Vec::with_capacity(array.len());
    for i in 0..array.len() {
        let bytes = array.get(i).ok_or("its FDArray has a hole")?;
        let dict = Dict::parse(bytes).ok_or("one of its Font DICTs is malformed")?;
        fds.push(private(data, &dict, 0)?);
    }
    if fds.is_empty() {
        return Err("its FDArray is empty".into());
    }
    Ok(fds)
}

/// Which Font DICT each glyph uses.
///
/// All three formats the specification defines are read. A format that is none of them is
/// refused rather than guessed at, because a wrong guess here picks the wrong subroutines
/// and draws a plausible wrong shape.
fn fd_select(data: &[u8], top: &Dict, num_glyphs: usize) -> Result<Vec<u16>, String> {
    let mut out = vec![0u16; num_glyphs];
    let Some(at) = top.number(KEY_FDSELECT).and_then(offset) else {
        // An FDArray with no FDSelect means one Font DICT for every glyph, which is what
        // the specification says.
        return Ok(out);
    };
    let format = *data.get(at).ok_or("its FDSelect is cut short")?;
    match format {
        0 => {
            // One Font DICT index per glyph, in order.
            for (gid, slot) in out.iter_mut().enumerate() {
                let at = at
                    .checked_add(1 + gid)
                    .ok_or("its FDSelect length overflows")?;
                *slot = u16::from(*data.get(at).ok_or("its FDSelect is cut short")?);
            }
        }
        // 3 and 4 are the same shape with different widths: a run of `first, fontDICTID`
        // records and then a sentinel one past the last glyph. A record's range runs from
        // its own `first` up to the *next* record's, which is why the fill happens when the
        // following record has been read.
        3 | 4 => {
            let wide = format == 4;
            let count_width = if wide { 4 } else { 2 };
            let id_width = if wide { 2 } else { 1 };
            let step = count_width + id_width;
            let ranges_at = at.checked_add(1).ok_or("its FDSelect length overflows")?;
            let ranges = wide_number(data, ranges_at, wide)?;
            let mut at = ranges_at
                .checked_add(count_width)
                .ok_or("its FDSelect length overflows")?;
            let mut start = 0usize;
            let mut fd = 0u16;
            for _ in 0..ranges {
                let first = wide_number(data, at, wide)?;
                let next = u16::from(
                    *data
                        .get(
                            at.checked_add(count_width)
                                .ok_or("its FDSelect length overflows")?,
                        )
                        .ok_or("its FDSelect is cut short")?,
                );
                at = at
                    .checked_add(step)
                    .ok_or("its FDSelect length overflows")?;
                let first = first.min(out.len());
                fill(&mut out, start, first, fd);
                start = first;
                fd = next;
            }
            let end = wide_number(data, at, wide)?.min(out.len());
            fill(&mut out, start, end, fd);
        }
        other => {
            return Err(format!(
                "its FDSelect is format {other}, which this does not read"
            ));
        }
    }
    Ok(out)
}

/// A glyph or range number in an `FDSelect`, which is 16 bits in format 3 and 32 in
/// format 4.
fn wide_number(data: &[u8], at: usize, wide: bool) -> Result<usize, String> {
    if wide {
        usize::try_from(be32(data, at).ok_or("its FDSelect is cut short")?)
            .map_err(|_| "its FDSelect holds a range number too large to walk".to_string())
    } else {
        Ok(usize::from(
            be16(data, at).ok_or("its FDSelect is cut short")?,
        ))
    }
}

fn fill(out: &mut [u16], from: usize, to: usize, value: u16) {
    for slot in out.get_mut(from..to).unwrap_or(&mut []) {
        *slot = value;
    }
}

/// Whether a CID font's charset says a character identifier *is* the glyph number.
///
/// The specification's rule is that a CID font with no charset maps identifiers to glyphs
/// by identity, and that is what nearly every CID font in a PDF does. The other case is a
/// charset written out; format 0 is the only one readable without expanding every range in
/// the file, and it is checked over its whole length rather than assumed from its first
/// entry.
fn identity_charset(data: &[u8], top: &Dict, num_glyphs: usize) -> bool {
    let Some(at) = top.number(KEY_CHARSET).and_then(offset) else {
        return true;
    };
    // 0, 1 and 2 are the predefined name-keyed charsets, so they map glyphs to *names*
    // rather than to identifiers. A CID font claiming one is claiming a mapping that is
    // not the identity.
    if at < 3 {
        return false;
    }
    // A range format is readable only by expanding every range in the file, and a font
    // whose charset is in that form is a font this does not claim to address.
    if data.get(at) != Some(&0) {
        return false;
    }
    // Format 0: one identifier per glyph after `.notdef`, in order, so the entry for glyph
    // `gid` sits at `at + 1 + (gid - 1) * 2`. The whole array is checked rather than the
    // first entry, and a charset cut short is not shown to be the identity either.
    (1..num_glyphs).all(|gid| {
        let at = at
            .saturating_add(1)
            .saturating_add(gid.saturating_sub(1) * 2);
        usize::from(be16(data, at).unwrap_or(u16::MAX)) == gid
    })
}

/// How many variation regions each item-variation record has.
///
/// `blend` pops one default per blended operand, that many deltas per region, and a count;
/// without the region count a `blend` cannot be told from the operator after it, so this is
/// not an optimisation. Only the counts are read: with no region scalars there is nothing
/// to interpolate *with*, and a blend's answer at the default instance is its default
/// value.
fn regions(data: &[u8], top: &Dict) -> Option<Vec<u32>> {
    let at = top.number(KEY_VARSTORE).and_then(offset)?;
    // ItemVariationStore: length, format, regionListOffset, dataCount, then one offset
    // per record. A store that cannot be read has no counts, which leaves a `blend` with
    // no region to work out how many operands it is eating — and a `blend` that guesses
    // would swallow the operator after it.
    let count_at = at.checked_add(8)?;
    let count = usize::from(be16(data, count_at)?);
    let offsets_at = count_at.checked_add(2)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let entry = offsets_at.checked_add(i.checked_mul(4)?)?;
        let item = at.checked_add(usize::try_from(be32(data, entry)?).ok()?)?;
        // ItemVariationStoreData: itemCount, shortDeltaCount, regionIndexCount.
        out.push(u32::from(be16(data, item.checked_add(4)?)?));
    }
    Some(out)
}

// ── the charstring interpreter ────────────────────────────────────────────────────

/// One token of a charstring.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    /// An operand.
    Number(f32),
    /// An operator in one byte.
    Op(u8),
    /// An operator in two bytes, the first of which was the escape.
    Escaped(u8),
    /// A mask operator, followed by bytes of mask this walk does nothing with.
    Mask,
}

/// One token and the length of the bytes it occupies.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spanned {
    token: Token,
    len: usize,
}

/// Read one token at `at`, and say how many bytes it took.
///
/// A truncated operand is an error rather than a number: the bytes after it are the next
/// token, and reading them as the tail of this one produces a program that runs.
fn token_at(code: &[u8], at: usize) -> Result<Spanned, String> {
    let b0 = be8(code, at)?;
    let (token, len) = match b0 {
        32..=246 => (Token::Number(f32::from(b0) - 139.0), 1),
        247..=250 => (
            Token::Number(f32::from(b0 - 247) * 256.0 + f32::from(be8(code, at + 1)?) + 108.0),
            2,
        ),
        251..=254 => (
            Token::Number(-(f32::from(b0 - 251) * 256.0 + f32::from(be8(code, at + 1)?) + 108.0)),
            2,
        ),
        28 => {
            let raw = be16(code, at + 1).ok_or_else(|| why("it is cut short"))?;
            (
                Token::Number(f32::from(i16::from_be_bytes(raw.to_be_bytes()))),
                3,
            )
        }
        255 => {
            let raw = be32(code, at + 1).ok_or_else(|| why("it is cut short"))?;
            let fixed = i32::from_be_bytes(raw.to_be_bytes()) as f32;
            (Token::Number(fixed / 65536.0), 5)
        }
        19 | 20 => (Token::Mask, 1),
        12 => (Token::Escaped(be8(code, at + 1)?), 2),
        // Every other byte below 32 is an operator, and two of them are the ones a DICT
        // spends on numbers instead: 29 is `callgsubr` here and a four-byte integer there,
        // and 30 is `vhcurveto` here and a real number there. Reading them as operand
        // encodings rejects most of a real font.
        other => (Token::Op(other), 1),
    };
    Ok(Spanned { token, len })
}

/// A subroutine being executed, and where in it the walk is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame<'c> {
    code: &'c [u8],
    at: usize,
}

/// What a finished walk reports.
#[derive(Debug, PartialEq)]
struct Walked {
    outline: Outline,
    /// The width in glyph units, or `None` for a CFF2 glyph, which has none.
    width: Option<f32>,
}

/// Executes one charstring.
struct Walker<'a, 'b> {
    cff: &'b Cff<'a>,
    local_subrs: Index<'a>,
    glyph: u32,
    segments: Vec<Segment>,
    stack: Vec<f32>,
    /// The pen, in glyph units rather than ems. The drawing operators are relative, and
    /// the matrix belongs where an absolute point becomes a segment.
    x: f32,
    y: f32,
    /// Where the contour being drawn began, and whether one is open. `None` means the pen
    /// is between contours, which is also when a `lineto` implies a `moveto` to where the
    /// pen is.
    start: Option<(f32, f32)>,
    /// How many stems have been declared, which is what says how long a hint mask is.
    hints: usize,
    /// How many bytes of mask the first `hintmask` asked for, once it has asked.
    hint_bytes: usize,
    /// Whether the width has been taken off the stack yet.
    width_taken: bool,
    width: Option<f32>,
    /// The variation store record the last `vsindex` chose.
    vsindex: usize,
    done: bool,
}

impl<'a, 'b> Walker<'a, 'b> {
    fn new(cff: &'b Cff<'a>, local_subrs: Index<'a>, glyph: u32) -> Self {
        Self {
            cff,
            local_subrs,
            glyph,
            segments: Vec::new(),
            stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            start: None,
            hints: 0,
            hint_bytes: 0,
            width_taken: false,
            width: None,
            vsindex: 0,
            done: false,
        }
    }

    /// Run a charstring, and report the outline and the width.
    fn walk(mut self, code: &'a [u8]) -> Result<Walked, String> {
        let mut frames = vec![Frame { code, at: 0 }];
        while !self.done {
            let Some(top) = frames.len().checked_sub(1) else {
                break;
            };
            let Some(frame) = frames.get(top).copied() else {
                break;
            };
            // Running off the end of a charstring or of a subroutine ends it. A CFF 1
            // charstring is supposed to finish with `endchar`, but a font whose last
            // operator is missing is a font whose shape is still knowable, and refusing it
            // would lose the glyph rather than report it.
            if frame.code.get(frame.at).is_none() {
                frames.pop();
                continue;
            }
            let spanned = token_at(frame.code, frame.at)?;
            let mut next = frame.at + spanned.len;
            // The mask is stepped first, because its length is not known until the operands
            // a mask may be carrying have been counted as stems.
            self.step(spanned.token, &mut frames)?;
            if spanned.token == Token::Mask {
                let mask = self.mask_bytes();
                if frame.code.len() < next.saturating_add(mask) {
                    return Err(format!(
                        "its hint mask claims {mask} bytes and only {} are left",
                        frame.code.len().saturating_sub(next)
                    ));
                }
                next = next.saturating_add(mask);
            }
            if let Some(frame) = frames.get_mut(top) {
                frame.at = next;
            }
        }
        self.close();
        Ok(Walked {
            outline: Outline {
                segments: self.segments,
            },
            width: self.width,
        })
    }

    // ── the stack ────────────────────────────────────────────────────────────────

    /// The whole stack, emptied.
    fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.stack)
    }

    fn push(&mut self, value: f32) -> Result<(), String> {
        if !value.is_finite() {
            return Err("one of its operands is not a number".into());
        }
        if self.stack.len() >= MAX_STACK {
            // Past the limit the format sets. The stack is bounded because a charstring
            // that grows it without bound is damage, not a long program.
            return Err(format!(
                "it leaves more than {MAX_STACK} values on the stack at once, which is past \
                 the limit the format sets"
            ));
        }
        self.stack.push(value);
        Ok(())
    }

    /// The whole stack with the width taken off the bottom of it, if there was one.
    ///
    /// A charstring may name its own advance width, as one extra operand before the first
    /// operator that clears the stack — written *before* the coordinates, so it is the
    /// bottom of the stack rather than the top. Whether the stack is *odd* or *even* when
    /// that happens depends on the operator: `rmoveto` and a stem declaration take pairs of
    /// coordinates and so expect an odd count, while `hmoveto` and `vmoveto` take one
    /// coordinate and so expect an even count. `even_odd` is that difference, and getting it
    /// the wrong way round offsets every glyph by its own width.
    fn take_with_width(&mut self, even_odd: usize) -> Vec<f32> {
        let args = std::mem::take(&mut self.stack);
        // A CFF2 charstring has no width at all, so there is nothing to take and nothing to
        // discard.
        if self.width_taken || self.cff.is_cff2() {
            return args;
        }
        self.width_taken = true;
        let (nominal, default) = self.cff.widths(self.glyph);
        if even_odd ^ (args.len() % 2) == 1 {
            self.width = Some(args.first().copied().unwrap_or(0.0) + nominal);
            args.into_iter().skip(1).collect()
        } else {
            self.width = Some(default);
            args
        }
    }

    // ── geometry ─────────────────────────────────────────────────────────────────

    /// A point in glyph units as a point in ems.
    // The six names are the specification's own for the six numbers of a matrix, which is
    // what a font matrix and a CTM both are.
    #[allow(clippy::many_single_char_names)]
    fn ems(&self, x: f32, y: f32) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.cff.font_matrix;
        (f64::from(a * x + c * y + e), f64::from(b * x + d * y + f))
    }

    fn move_rel(&mut self, dx: f32, dy: f32) {
        self.close();
        self.x += dx;
        self.y += dy;
        self.start = Some((self.x, self.y));
        let (x, y) = self.ems(self.x, self.y);
        self.segments.push(Segment::Move(x, y));
    }

    fn line_rel(&mut self, dx: f32, dy: f32) {
        self.begin_if_needed();
        self.x += dx;
        self.y += dy;
        let (x, y) = self.ems(self.x, self.y);
        self.segments.push(Segment::Line(x, y));
    }

    /// One curve, from three offsets.
    ///
    /// **Each of the three points is an offset from the point before it**, not from where
    /// the pen was: the first from the current point, the second from the first, and the
    /// third from the second. This is the single easiest thing to get wrong in the whole
    /// interpreter, because the wrong reading produces a closed, plausible, wrong-shaped
    /// glyph rather than an error — a serif face draws every stem too narrow and every
    /// curve too flat, and nothing reports it. The spec is explicit: "the first specified
    /// point is relative to the current point, the second is relative to the first, and the
    /// third is relative to the second."
    ///
    /// The matrix goes on at the end, once each point is absolute, because the offsets are
    /// not affected by it.
    fn curve_rel(&mut self, dx1: f32, dy1: f32, dx2: f32, dy2: f32, dx3: f32, dy3: f32) {
        self.begin_if_needed();
        let x1 = self.x + dx1;
        let y1 = self.y + dy1;
        let x2 = x1 + dx2;
        let y2 = y1 + dy2;
        self.x = x2 + dx3;
        self.y = y2 + dy3;
        let (ax, ay) = self.ems(x1, y1);
        let (bx, by) = self.ems(x2, y2);
        let (cx, cy) = self.ems(self.x, self.y);
        self.segments.push(Segment::Curve(ax, ay, bx, by, cx, cy));
    }

    /// A `lineto` or a curve before any `moveto` starts a contour where the pen is.
    ///
    /// The specification does not allow it, and every implementation that draws such a glyph
    /// draws it from where the pen is. Refusing it would leave the pen's position undefined
    /// for everything after it.
    fn begin_if_needed(&mut self) {
        if self.start.is_none() {
            self.start = Some((self.x, self.y));
            let (x, y) = self.ems(self.x, self.y);
            self.segments.push(Segment::Move(x, y));
        }
    }

    /// Close the contour being drawn, if one is open.
    ///
    /// A CFF contour is closed by the fill rule rather than by an operator: there is no
    /// `closepath`, so the closing edge has to exist or the winding comes out with a notch
    /// in it. The next `moveto` is what closes the last one.
    fn close(&mut self) {
        let Some((sx, sy)) = self.start.take() else {
            return;
        };
        let (sx, sy) = self.ems(sx, sy);
        let (ex, ey) = self.ems(self.x, self.y);
        // Already back where it started: the edge is there, and adding it again would be a
        // figure of eight.
        if (ex - sx).abs() > 1e-9 || (ey - sy).abs() > 1e-9 {
            self.segments.push(Segment::Line(sx, sy));
        }
    }

    /// How many bytes of hint mask follow a `hintmask` or `cntrmask`.
    ///
    /// One bit per stem, rounded up to whole bytes: `1 + floor((stems - 1) / 8)`. The
    /// length is worked out at the *first* mask and reused afterwards, because every stem
    /// is declared before it and a font that re-asked would answer differently once a
    /// second mask with no operands of its own came along.
    ///
    /// A first mask with no stems at all re-asks, because a zero-byte mask carries no
    /// information about whether the answer was worked out or not.
    fn mask_bytes(&mut self) -> usize {
        if self.hint_bytes == 0 {
            self.hint_bytes = self.hints.div_ceil(8);
        }
        self.hint_bytes
    }

    // ── one token ────────────────────────────────────────────────────────────────

    fn step(&mut self, token: Token, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        match token {
            Token::Number(v) => self.push(v),
            Token::Mask => {
                // A mask is hinting and this walk has no grid to snap to, so the bits
                // themselves are skipped. What they are for is the stem count, and that is
                // not only what the declarations before it said: the format lets a font
                // leave out a `vstemhm` whose definitions are followed straight by a mask,
                // leaving its operands on the stack for the mask to claim. Reading that as
                // a mask of the wrong length desynchronises everything after it.
                let args = self.take_with_width(0);
                self.hints += args.len() / 2;
                Ok(())
            }
            Token::Op(op) => self.operator(op, frames),
            Token::Escaped(op) => self.escaped(op),
        }
    }

    fn operator(&mut self, op: u8, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        match op {
            1 | 3 | 18 | 23 => {
                // hstem, vstem, hstemhm, vstemhm: pairs of stem positions.
                let args = self.take_with_width(0);
                self.hints += args.len() / 2;
                Ok(())
            }
            4 => {
                // vmoveto
                let args = self.take_with_width(1);
                let dy = one(&args, "vmoveto")?;
                self.move_rel(0.0, dy);
                Ok(())
            }
            22 => {
                // hmoveto
                let args = self.take_with_width(1);
                let dx = one(&args, "hmoveto")?;
                self.move_rel(dx, 0.0);
                Ok(())
            }
            21 => {
                // rmoveto
                let args = self.take_with_width(0);
                let (dx, dy) = pair(&args, "rmoveto")?;
                self.move_rel(dx, dy);
                Ok(())
            }
            5 => {
                // rlineto
                let args = self.take();
                whole(&args, 2, "rlineto")?;
                for chunk in args.chunks_exact(2) {
                    if let [a, b] = chunk {
                        self.line_rel(*a, *b);
                    }
                }
                Ok(())
            }
            6 | 7 => {
                // hlineto, vlineto: alternating, starting with whichever was named.
                let args = self.take();
                let horizontal = op == 6;
                for (i, v) in args.iter().enumerate() {
                    if horizontal == (i % 2 == 0) {
                        self.line_rel(*v, 0.0);
                    } else {
                        self.line_rel(0.0, *v);
                    }
                }
                Ok(())
            }
            8 => {
                // rrcurveto
                let args = self.take();
                whole(&args, 6, "rrcurveto")?;
                for chunk in args.chunks_exact(6) {
                    if let [a, b, c, d, e, f] = chunk {
                        self.curve_rel(*a, *b, *c, *d, *e, *f);
                    }
                }
                Ok(())
            }
            24 => {
                // rcurveline: whole curves, then one line.
                let args = self.take();
                let at = args
                    .len()
                    .checked_sub(2)
                    .ok_or("rcurveline has no room for its line")?;
                let (curves, line) = args.split_at(at);
                whole(curves, 6, "rcurveline")?;
                for chunk in curves.chunks_exact(6) {
                    if let [a, b, c, d, e, f] = chunk {
                        self.curve_rel(*a, *b, *c, *d, *e, *f);
                    }
                }
                if let [a, b] = *line {
                    self.line_rel(a, b);
                }
                Ok(())
            }
            25 => {
                // rlinecurve: whole lines, then one curve.
                let args = self.take();
                let at = args
                    .len()
                    .checked_sub(6)
                    .ok_or("rlinecurve has no room for its curve")?;
                let (lines, curve) = args.split_at(at);
                whole(lines, 2, "rlinecurve")?;
                for chunk in lines.chunks_exact(2) {
                    if let [a, b] = chunk {
                        self.line_rel(*a, *b);
                    }
                }
                if let [a, b, c, d, e, f] = *curve {
                    self.curve_rel(a, b, c, d, e, f);
                }
                Ok(())
            }
            26 => {
                // vvcurveto: an optional leading dx, then curves of (dy1 dx2 dy2 dy3).
                let taken = self.take();
                let (mut dx, args) = odd_leading(&taken);
                whole(args, 4, "vvcurveto")?;
                for chunk in args.chunks_exact(4) {
                    if let [dya, dxb, dyb, dyc] = chunk {
                        self.curve_rel(dx, *dya, *dxb, *dyb, 0.0, *dyc);
                    }
                    dx = 0.0;
                }
                Ok(())
            }
            27 => {
                // hhcurveto: an optional leading dy, then curves of (dx1 dx2 dy2 dx3).
                let taken = self.take();
                let (mut dy, args) = odd_leading(&taken);
                whole(args, 4, "hhcurveto")?;
                for chunk in args.chunks_exact(4) {
                    if let [dxa, dxb, dyb, dxc] = chunk {
                        self.curve_rel(*dxa, dy, *dxb, *dyb, *dxc, 0.0);
                    }
                    dy = 0.0;
                }
                Ok(())
            }
            30 | 31 => {
                // vhcurveto, hvcurveto: alternating, with an optional fifth operand on the
                // last curve of the run for the final coordinate that would otherwise be
                // zero.
                let mut args = self.take();
                let mut vertical = op == 30;
                while !args.is_empty() {
                    if vertical {
                        self.alternating_curve(&mut args, true)?;
                    } else {
                        self.alternating_curve(&mut args, false)?;
                    }
                    vertical = !vertical;
                }
                Ok(())
            }
            10 => enter(&mut self.stack, &self.local_subrs, frames, "callsubr"),
            29 => enter(&mut self.stack, &self.cff.global_subrs, frames, "callgsubr"),
            11 => {
                // `return` ends the innermost subroutine. Returning from the charstring
                // itself ends it too, which the loop above notices.
                frames.pop();
                Ok(())
            }
            14 => self.end_char(),
            15 => {
                // vsindex: choose a variation store record. With no store there is nothing
                // to choose, and the value is remembered and never read.
                let args = self.take();
                self.vsindex = one(&args, "vsindex")?.max(0.0) as usize;
                Ok(())
            }
            16 => self.blend(),
            // The specification says an operator a reader does not know is ignored with
            // the stack cleared, and every reference reader does that rather than
            // refusing the glyph. Byte 0 is not an instruction at all and lands here.
            _ => {
                let _ = self.take();
                Ok(())
            }
        }
    }

    fn escaped(&mut self, op: u8) -> Result<(), String> {
        match op {
            // dotsection, from a handful of early OpenType CFF fonts. It is a hinting
            // instruction, and there is nothing left for it to hint.
            0 => Ok(()),
            34 => {
                // hflex: two curves. The first three of the seven operands are horizontal
                // deltas, and the last three have their middle one bent back by the height
                // of the second.
                let args = self.take();
                let [dx1, dx2, dy2, dx3, dx4, dx5, dx6] = at(&args, "hflex")? else {
                    return Err("`hflex` was given too few operands".into());
                };
                self.curve_rel(*dx1, 0.0, *dx2, *dy2, *dx3, 0.0);
                self.curve_rel(*dx4, 0.0, *dx5, -*dy2, *dx6, 0.0);
                Ok(())
            }
            35 => {
                // flex: two whole curves, and a flex depth this does not use.
                let args = self.take();
                let [
                    dx1,
                    dy1,
                    dx2,
                    dy2,
                    dx3,
                    dy3,
                    dx4,
                    dy4,
                    dx5,
                    dy5,
                    dx6,
                    dy6,
                    ..,
                ] = at(&args, "flex")?
                else {
                    return Err("`flex` was given too few operands".into());
                };
                self.curve_rel(*dx1, *dy1, *dx2, *dy2, *dx3, *dy3);
                self.curve_rel(*dx4, *dy4, *dx5, *dy5, *dx6, *dy6);
                Ok(())
            }
            36 => {
                // hflex1: the first curve is flat and the second one closes the gap in y.
                let args = self.take();
                let [dx1, dy1, dx2, dy2, dx3, dx4, dx5, dy5, dx6] = at(&args, "hflex1")? else {
                    return Err("`hflex1` was given too few operands".into());
                };
                let dy6 = -(*dy1 + *dy2 + *dy5);
                self.curve_rel(*dx1, *dy1, *dx2, *dy2, *dx3, 0.0);
                self.curve_rel(*dx4, 0.0, *dx5, *dy5, *dx6, dy6);
                Ok(())
            }
            37 => {
                // flex1: whichever axis moved least is the one the last curve finishes on.
                let args = self.take();
                let [dx1, dy1, dx2, dy2, dx3, dy3, dx4, dy4, dx5, dy5, last] = at(&args, "flex1")?
                else {
                    return Err("`flex1` was given too few operands".into());
                };
                let dx = *dx1 + *dx2 + *dx3 + *dx4 + *dx5;
                let dy = *dy1 + *dy2 + *dy3 + *dy4 + *dy5;
                let (dx6, dy6) = if dx.abs() > dy.abs() {
                    (*last, -dy)
                } else {
                    (-dx, *last)
                };
                self.curve_rel(*dx1, *dy1, *dx2, *dy2, *dx3, *dy3);
                self.curve_rel(*dx4, *dy4, *dx5, *dy5, dx6, dy6);
                Ok(())
            }
            other => Err(format!(
                "escaped operator 12 {other} is one this does not execute"
            )),
        }
    }

    /// One curve of a `vhcurveto` or an `hvcurveto`, which take four operands and whose
    /// last curve may take a fifth for the trailing coordinate that would otherwise be zero.
    fn alternating_curve(&mut self, args: &mut Vec<f32>, vertical: bool) -> Result<(), String> {
        // Four operands, or five on the *last* curve of the run — so the count is not a
        // multiple of four and must not be asked to be. Anything shorter is damage.
        if args.len() < 4 {
            return Err(format!(
                "`{}` was given {} operands, and a curve takes four",
                if vertical { "vhcurveto" } else { "hvcurveto" },
                args.len()
            ));
        }
        let last = args.len() == 5;
        let [a, b, c, d] = args.get(..4).unwrap_or_default() else {
            return Err("an alternating curve was given fewer than four operands".into());
        };
        let (a, b, c, d) = (*a, *b, *c, *d);
        // The fifth operand belongs to the last curve of the run and to no other. Reading it
        // on the first of a pair would take the *next* curve's first operand as this one's
        // trailing coordinate, which moves the end of every curve in a multi-curve run.
        let extra = if last {
            args.get(4).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        args.drain(..if last { 5 } else { 4 });
        if vertical {
            // The four operands are (dy1 dx2 dy2 dy3), so the first control point is at
            // (0, dy1) and the third is at (0, dy3).
            self.curve_rel(0.0, a, b, c, d, extra);
        } else {
            // The four are (dx1 dx2 dy2 dy3), so the first control point is at (dx1, 0) and
            // the third is at (dx3, dy3) — which is why the fifth operand is the *x* of the
            // end point and the fourth is its *y*, the other way round from `vcurveto`.
            self.curve_rel(a, 0.0, b, c, extra, d);
        }
        Ok(())
    }

    /// `endchar`: the glyph is finished, and the stack must be empty.
    fn end_char(&mut self) -> Result<(), String> {
        let args = self.take_with_width(0);
        if args.len() == 4 || args.len() == 5 {
            // `seac`: build an accented character out of two others, named rather than
            // numbered. Drawing the unaccented one instead would be a wrong shape that
            // looks like a right one.
            return Err(
                "it builds an accented character with `seac`, which names two glyphs this \
                 does not resolve"
                    .into(),
            );
        }
        if !args.is_empty() {
            // The format requires the stack to be empty here, and a charstring that leaves
            // values on it has been truncated or mis-decoded. Drawing the shape up to this
            // point would be drawing a glyph that is not the glyph.
            return Err(format!(
                "it leaves {} value(s) on the stack at `endchar`, which the format does not \
                 allow",
                args.len()
            ));
        }
        self.close();
        self.done = true;
        Ok(())
    }

    /// `blend`: replace each default value with itself.
    ///
    /// A blend is one default per blended operand, then one delta per *variation region*
    /// for each of them, then a count of how many were blended. With no region scalars to
    /// add, the answer at the default instance is the default value, so the deltas are
    /// discarded and the defaults kept — which is the default instance of a variable font,
    /// and is what a PDF asks for. A font at a named location is not representable here
    /// and is not claimed.
    fn blend(&mut self) -> Result<(), String> {
        let args = self.take();
        let Some(&count) = args.last() else {
            return Err("`blend` has nothing on the stack".into());
        };
        let count = count.max(0.0) as usize;
        // With no store at all there are no regions and so no deltas, which is what a CFF2
        // table without a store means. A store that is present but unreadable leaves the
        // count unknown, and a guess would swallow the operator after this one.
        let regions = match &self.cff.regions {
            None => 0usize,
            Some(known) => known
                .get(self.vsindex)
                .copied()
                .ok_or("it chooses a variation record its store does not have")?
                as usize,
        };
        let deltas = count
            .checked_mul(regions)
            .ok_or("`blend` was given a count that overflows")?;
        let needed = count
            .checked_add(deltas)
            .and_then(|n| n.checked_add(1))
            .ok_or("`blend` was given a count that overflows")?;
        if args.len() < needed {
            return Err(format!(
                "`blend` was given {} operands for {count} blend(s) over {regions} variation \
                 region(s), which is fewer than it needs",
                args.len().saturating_sub(1)
            ));
        }
        for value in args.into_iter().take(count) {
            self.push(value)?;
        }
        Ok(())
    }
}

/// Enter a subroutine, subject to the nesting limit.
///
/// `stack` and `subrs` are separate arguments rather than two fields of the walker because
/// taking the operand empties the stack while the subroutines are being read from it, and
/// two fields of one value can be borrowed at once where a method and its receiver cannot.
fn enter<'a>(
    stack: &mut Vec<f32>,
    subrs: &Index<'a>,
    frames: &mut Vec<Frame<'a>>,
    name: &str,
) -> Result<(), String> {
    // Exactly one value comes off the stack: the subroutine number. The rest is the
    // subroutine's to use, and it is common for a subroutine to begin with a `moveto` that
    // takes the values the caller pushed before calling it.
    let Some(number) = stack.pop() else {
        return Err(format!("`{name}` has nothing on the stack to call"));
    };
    // The operand is a subroutine *number* only once the bias is added, and the bias
    // depends on how many subroutines there are. The wrong bias at a boundary does not
    // fail: it lands on a different subroutine and draws a plausible wrong shape.
    if !number.is_finite() || number.fract() != 0.0 {
        return Err(format!(
            "`{name}` was given {number}, which is not a subroutine number"
        ));
    }
    if frames.len() > MAX_DEPTH {
        // The charstring itself is the first frame, so this is the limit on how many calls
        // may be nested inside it.
        return Err(format!(
            "it nests more than {MAX_DEPTH} subroutine calls, which is past the limit the \
             format sets"
        ));
    }
    let wanted = number as i64;
    let index = wanted.saturating_add(Cff::subr_bias(subrs.len()) as i64);
    let index = usize::try_from(index.max(0)).map_err(|_| "its subroutine number overflows")?;
    let code = subrs.get(index).ok_or_else(|| {
        format!(
            "`{name}` asks for subroutine {wanted} of {}, which it does not have",
            subrs.len()
        )
    })?;
    frames.push(Frame { code, at: 0 });
    Ok(())
}

/// An operator's operands as a slice, insisting there are some.
fn at<'f>(args: &'f [f32], name: &str) -> Result<&'f [f32], String> {
    if args.is_empty() {
        Err(format!("`{name}` was given no operands"))
    } else {
        Ok(args)
    }
}

/// An operator's operands, insisting there is at least one.
fn one(args: &[f32], name: &str) -> Result<f32, String> {
    args.first()
        .copied()
        .ok_or_else(|| format!("`{name}` was given no operands"))
}

/// An operator's operands, insisting there are at least two.
fn pair(args: &[f32], name: &str) -> Result<(f32, f32), String> {
    match args {
        [a, b, ..] => Ok((*a, *b)),
        _ => Err(format!("`{name}` needs two operands")),
    }
}

/// Insist the operands divide into whole groups, and that there are some.
///
/// A truncated group is a dropped coordinate. Drawing the whole groups and ignoring the
/// remainder is how a glyph comes out missing a corner with nothing reporting it.
fn whole(args: &[f32], size: usize, name: &str) -> Result<(), String> {
    if args.is_empty() {
        return Err(format!("`{name}` was given no operands"));
    }
    if args.len() % size != 0 {
        return Err(format!(
            "`{name}` was given {} operands, which is not a whole number of {size}-operand \
             groups",
            args.len()
        ));
    }
    Ok(())
}

/// Split off a leading value from a list whose length is odd, which is where the optional
/// first operand of `vvcurveto` and `hhcurveto` lives.
fn odd_leading(args: &[f32]) -> (f32, &[f32]) {
    if args.len() % 2 == 1 {
        (
            args.first().copied().unwrap_or(0.0),
            args.get(1..).unwrap_or_default(),
        )
    } else {
        (0.0, args)
    }
}

// ── finding the CFF in a font program ─────────────────────────────────────────────

/// The CFF table inside a font program, if the program carries one.
///
/// A PDF embeds a CFF font in two shapes. A `/FontFile3` with `/Subtype /Type1C` is the
/// bare table, and so is a Type 1 font's `FontFile`; a `/FontFile3` with `/Subtype
/// /OpenType` is a whole `sfnt` whose `CFF ` or `CFF2` table holds the outlines. They are
/// the same table once it has been found, and a reader handling only one of them would
/// draw half the embedded fonts in the world.
#[must_use]
pub fn cff_bytes(data: &[u8]) -> Option<&[u8]> {
    // A bare table starts with its own major version. An `sfnt` starts with its own, and
    // those are `OTTO`, `true`, `ttcf` or `0x00010000` — never one or two in the first
    // byte, which is what tells the two apart.
    match data.first() {
        Some(1 | 2) => Some(data),
        _ => {
            let face = Face::parse(data, 0).ok()?;
            let raw = face.raw_face();
            raw.table(Tag::from_bytes(b"CFF "))
                .or_else(|| raw.table(Tag::from_bytes(b"CFF2")))
        }
    }
}

/// How many units a CFF font's own grid has to the em.
///
/// A bare CFF program carries no `head` table, so the em comes from its `FontMatrix`: the
/// matrix says how much of an em a glyph unit is, and the reciprocal is the grid. The
/// default matrix is a thousandth, which is a thousand-unit grid — the same answer PDF's
/// own `/Widths` are written in.
#[must_use]
pub fn units_from_matrix(matrix: &[f32; 6]) -> u16 {
    let scale = f64::from(matrix.first().copied().unwrap_or(0.0));
    if !scale.is_finite() || scale.abs() < f64::EPSILON {
        return DEFAULT_UNITS_PER_EM;
    }
    (1.0 / scale).round().clamp(1.0, f64::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and read the font programs they were
    // built from. Both are what a test is for: the panic-free rule is about what the
    // product does with a file from the internet, not about how a test reads a font built
    // two functions above it.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::float_cmp
    )]

    use super::*;

    // ── building a CFF, here ────────────────────────────────────────────────────
    //
    // Every test below needs a font program, and a real one is somebody else's work that
    // may or may not be installed. So these tests build their own, a byte at a time, from
    // the format rather than from a copy. That is also the only way to be sure of what a
    // test is exercising: an outline compared against a stored snapshot proves only that
    // the snapshot has not changed, and a subroutine bias checked at one count inside a
    // range cannot see an off-by-one at the boundary.

    /// One glyph's charstring, written the way a writer writes it.
    ///
    /// Each builder takes and returns, so a program reads as the list of things it says.
    #[derive(Debug, Default, Clone)]
    struct Charstring {
        bytes: Vec<u8>,
    }

    impl Charstring {
        fn new() -> Self {
            Self::default()
        }

        /// An operand, in whichever of the format's encodings is shortest.
        fn arg(mut self, value: i32) -> Self {
            match value {
                -107..=107 => self.bytes.push((value + 139) as u8),
                108..=1131 => {
                    let v = value - 108;
                    self.bytes.push((v / 256 + 247) as u8);
                    self.bytes.push((v % 256) as u8);
                }
                -1131..=-108 => {
                    let v = -value - 108;
                    self.bytes.push((v / 256 + 251) as u8);
                    self.bytes.push((v % 256) as u8);
                }
                _ => {
                    self.bytes.push(28);
                    self.bytes
                        .extend_from_slice(&i16::try_from(value).unwrap().to_be_bytes());
                }
            }
            self
        }

        /// An operator in one byte.
        fn op(mut self, op: u8) -> Self {
            self.bytes.push(op);
            self
        }

        /// An operator in two bytes, the first of which is the escape.
        fn escape(mut self, op: u8) -> Self {
            self.bytes.push(12);
            self.bytes.push(op);
            self
        }

        /// Bytes that follow an operator without being read as one, which is what a hint
        /// mask is.
        fn raw(mut self, bytes: &[u8]) -> Self {
            self.bytes.extend_from_slice(bytes);
            self
        }

        /// A subroutine that calls the one after it and returns, so a chain of them is one
        /// level of nesting each.
        fn chain(to: usize) -> Self {
            Self::new().arg(to as i32 - 107).op(10).op(11)
        }

        fn build(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// An INDEX, with the 16-bit count CFF 1 writes.
    /// An INDEX, with the 16-bit count CFF 1 writes. An empty one is two bytes, which is
    /// the reader's `len_of` in a different costume.
    fn index(items: &[Vec<u8>]) -> Vec<u8> {
        indexed(items, 2)
    }

    /// An INDEX, with whichever count width `count_size` names: 2 for CFF 1, 4 for CFF 2.
    fn indexed(items: &[Vec<u8>], count_size: usize) -> Vec<u8> {
        let mut out = Vec::new();
        if items.is_empty() {
            // An empty INDEX is a count of zero and nothing else.
            out.extend_from_slice(&count_bytes(0, count_size));
            return out;
        }
        let total: usize = items.iter().map(Vec::len).sum::<usize>() + 1;
        let width = if total < 0x100 {
            1
        } else if total < 0x10000 {
            2
        } else if total < 0x1000000 {
            3
        } else {
            4
        };
        out.extend_from_slice(&count_bytes(items.len(), count_size));
        out.push(width as u8);
        let mut at = 1usize;
        out.extend_from_slice(&offset(at, width));
        for item in items {
            at += item.len();
            out.extend_from_slice(&offset(at, width));
        }
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    fn count_bytes(count: usize, count_size: usize) -> Vec<u8> {
        if count_size == 2 {
            u16::try_from(count).unwrap().to_be_bytes().to_vec()
        } else {
            u32::try_from(count).unwrap().to_be_bytes().to_vec()
        }
    }

    fn offset(value: usize, width: usize) -> Vec<u8> {
        u32::try_from(value).unwrap().to_be_bytes()[4 - width..].to_vec()
    }

    /// A DICT operand in the three-byte form whatever its value.
    ///
    /// Fixed width so a table's length is known before its contents are, which is what lets
    /// the tables below be laid out by arithmetic rather than by patching bytes into a
    /// finished file.
    fn fixed(value: usize) -> Vec<u8> {
        let v = u32::try_from(value).unwrap();
        vec![28, v.to_be_bytes()[2], v.to_be_bytes()[3]]
    }

    /// What a test wants in a name-keyed font.
    struct Spec {
        glyphs: Vec<Vec<u8>>,
        local_subrs: Vec<Vec<u8>>,
        global_subrs: Vec<Vec<u8>>,
        nominal_width_x: i32,
        default_width_x: i32,
        /// `Some(1)` makes the Top DICT claim Type 1 charstrings, which is the one dialect
        /// this refuses.
        charstring_type: Option<i32>,
    }

    impl Spec {
        /// One glyph, no subroutines, and the widths a CFF font declares.
        fn one(glyph: Charstring) -> Self {
            Self {
                glyphs: vec![glyph.build()],
                local_subrs: Vec::new(),
                global_subrs: Vec::new(),
                nominal_width_x: 0,
                default_width_x: 500,
                charstring_type: None,
            }
        }

        /// One glyph and some local subroutines, which is what the bias tests need.
        fn with_subrs(glyph: Charstring, local_subrs: Vec<Vec<u8>>) -> Self {
            Self {
                local_subrs,
                ..Self::one(glyph)
            }
        }

        fn widths(mut self, nominal: i32, default: i32) -> Self {
            self.nominal_width_x = nominal;
            self.default_width_x = default;
            self
        }
    }

    /// A name-keyed CFF 1 font: header, Name, Top DICT, String and global subroutines,
    /// and then the tables the Top DICT names by offset.
    fn cff1(spec: &Spec) -> Vec<u8> {
        let charstrings = index(&spec.glyphs);
        let gsubrs = index(&spec.global_subrs);
        let lsubrs = index(&spec.local_subrs);

        // The Private DICT, and the local subroutines straight after it: `/Subrs` is an
        // offset from the Private DICT's own first byte, which is the one offset in this
        // format that is not measured from the start of the file.
        let mut private = Vec::new();
        private.extend(fixed(spec.default_width_x as usize));
        private.push(20); // defaultWidthX
        private.extend(fixed(spec.nominal_width_x as usize));
        private.push(21); // nominalWidthX
        // `/Subrs` is the offset of the local subroutines from the first byte of this
        // Private DICT, and they follow it, so the offset is this DICT's own length.
        let subrs_at = private.len() + 4;
        private.extend(fixed(subrs_at));
        private.push(19); // Subrs

        // Every Top DICT operand is three bytes wide, so the Top DICT's length is known
        // whichever font this is — 4 for CharStrings, 7 for Private, and 4 more for a
        // CharstringType — and the tables below can be placed by arithmetic rather than by
        // patching bytes into a finished file.
        let top_len = 11 + if spec.charstring_type.is_some() { 5 } else { 0 };
        let top_index = index(&[vec![0u8; top_len]]);
        let top_at = 6;
        let string_at = top_at + top_index.len();
        let charstrings_at = string_at + 2 + gsubrs.len();
        let private_at = charstrings_at + charstrings.len();

        let mut top = Vec::new();
        if let Some(kind) = spec.charstring_type {
            top.extend(fixed(kind as usize));
            top.extend([12, 6]); // CharstringType
        }
        top.extend(fixed(charstrings_at));
        top.push(17); // CharStrings
        top.extend(fixed(private.len()));
        top.extend(fixed(private_at));
        top.push(18); // Private
        assert_eq!(
            top.len(),
            top_len,
            "the Top DICT length is what it was sized for"
        );

        let mut out = vec![1u8, 0, 4, 4]; // major, minor, hdrSize, offSize
        out.extend_from_slice(&index(&[])); // Name INDEX
        out.extend_from_slice(&index(&[top]));
        out.extend_from_slice(&index(&[])); // String INDEX
        out.extend_from_slice(&gsubrs);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&private);
        out.extend_from_slice(&lsubrs);
        out
    }

    /// What a test wants in a CID-keyed font: two Font DICTs, so that which subroutines a
    /// glyph may call is a property of the glyph.
    struct CidSpec {
        glyphs: Vec<Vec<u8>>,
        /// One Font DICT each: its nominal width, its default width, and its subroutines.
        fds: Vec<(i32, i32, Vec<Vec<u8>>)>,
        /// `None` for the identity charset, which is a charset that is not written out.
        /// `Some(bytes)` is the charset table, whatever it says.
        charset: Option<Vec<u8>>,
    }

    /// How many bytes an INDEX of `count` items totalling `total` bytes occupies.
    ///
    /// Needed before the items are written, so that a table *after* an INDEX can be placed
    /// by arithmetic rather than by patching bytes into a finished file.
    fn index_len(count: usize, total: usize, count_size: usize) -> usize {
        if count == 0 {
            return count_size;
        }
        let end = total + 1;
        let width = if end < 0x100 {
            1
        } else if end < 0x10000 {
            2
        } else if end < 0x1000000 {
            3
        } else {
            4
        };
        count_size + 1 + (count + 1) * width + total
    }

    /// A CID-keyed CFF 1 font: the same four INDEXes, plus a charset, an FDArray of Font
    /// DICTs with their Private DICTs, and an FDSelect.
    fn cff1_cid(spec: &CidSpec) -> Vec<u8> {
        let charstrings = index(&spec.glyphs);
        let gsubrs = index(&[]);
        let num_glyphs = spec.glyphs.len();

        // ROS, charset, FDArray, FDSelect and CharStrings: each a three-byte operand and a
        // one-byte operator, except ROS which is four operands.
        const TOP_LEN: usize = 14 + 4 + 5 + 5 + 4;
        let top_index = index(&[vec![0u8; TOP_LEN]]);
        let charstrings_at = 6 + top_index.len() + 2 + gsubrs.len();

        // Format 0 is one identifier per glyph in order, which is the identity mapping.
        let mut charset = Vec::new();
        match &spec.charset {
            None => {
                charset.push(0);
                for gid in 1..num_glyphs {
                    charset.extend_from_slice(&u16::try_from(gid).unwrap().to_be_bytes());
                }
            }
            Some(written) => charset.extend_from_slice(written),
        }
        let charset_at = charstrings_at + charstrings.len();
        let fdarray_at = charset_at + charset.len();

        // Every Font DICT is a Private DICT's size and offset; every Private DICT is two
        // widths and an offset; and every set of subroutines follows its Private DICT.
        // Each Font DICT is exactly seven bytes, so the FDArray's own length is known
        // before any of them is written and the Private DICTs behind it can be placed.
        const FONT_DICT_LEN: usize = 7;
        let fds_base = fdarray_at + index_len(spec.fds.len(), FONT_DICT_LEN * spec.fds.len(), 2);

        let mut font_dicts = Vec::new();
        let mut body: Vec<u8> = Vec::new();
        let mut body_len = 0usize;
        for (nominal, default, subrs) in &spec.fds {
            let subrs_index = index(subrs);
            let mut private = Vec::new();
            private.extend(fixed(*nominal as usize));
            private.push(21); // nominalWidthX
            private.extend(fixed(*default as usize));
            private.push(20); // defaultWidthX
            let subrs_at = private.len() + 4;
            private.extend(fixed(subrs_at));
            private.push(19); // Subrs
            let at = fds_base + body_len;
            body.extend_from_slice(&private);
            body.extend_from_slice(&subrs_index);
            body_len += private.len() + subrs_index.len();
            let mut dict = Vec::new();
            dict.extend(fixed(private.len()));
            dict.extend(fixed(at));
            dict.push(18); // Private
            font_dicts.push(dict);
        }
        let fdarray = index(&font_dicts);
        assert_eq!(
            fdarray.len(),
            index_len(spec.fds.len(), FONT_DICT_LEN * spec.fds.len(), 2)
        );
        let fdarray_len = fdarray.len() + body_len;
        let fdselect_at = fdarray_at + fdarray_len;

        // Format 0 is one Font DICT index per glyph, in order: the first glyph uses the
        // first FD and the second the second, which is the case that has to be read.
        let mut fdselect = vec![0u8];
        fdselect.extend((0..num_glyphs).map(|i| u8::try_from(i % 254).unwrap()));

        let mut top = Vec::new();
        // ROS is registry, ordering and supplement, and the first two are string
        // identifiers: 390 is Adobe and 391 is Identity, the ordering that maps an
        // identifier to a glyph number, which is the only one this addresses.
        top.extend(fixed(390));
        top.extend(fixed(391));
        top.extend(fixed(0));
        top.extend(fixed(charset_at));
        top.extend([12, 30]); // ROS
        top.extend(fixed(charset_at));
        top.push(15); // charset
        top.extend(fixed(fdarray_at));
        top.extend([12, 36]); // FDArray
        top.extend(fixed(fdselect_at));
        top.extend([12, 37]); // FDSelect
        top.extend(fixed(charstrings_at));
        top.push(17); // CharStrings
        assert_eq!(
            top.len(),
            TOP_LEN,
            "the Top DICT length is what it was sized for"
        );

        let mut out = vec![1u8, 0, 4, 4];
        out.extend_from_slice(&index(&[])); // Name INDEX
        out.extend_from_slice(&index(&[top]));
        out.extend_from_slice(&index(&[])); // String INDEX
        out.extend_from_slice(&gsubrs);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(&charset);
        out.extend_from_slice(&fdarray);
        out.extend_from_slice(&body);
        out.extend_from_slice(&fdselect);
        out
    }

    /// Compare an outline against hand-written segments, to the precision the format's
    /// arithmetic really has.
    ///
    /// A charstring's coordinates are `f32`, so 100 units of a thousand-unit em is
    /// 0.10000000149011612 and not 0.1. Asserting exact equality would make every test in
    /// this module fail for the right reason and the wrong one; asserting a loose
    /// tolerance would let a real error through. A millionth of an em is a millionth of a
    /// pixel at any size a page uses, and a mistake in this walk is orders of magnitude
    /// bigger than that.
    #[track_caller]
    #[allow(clippy::many_single_char_names)]
    fn assert_outline(got: &Outline, want: &[Segment]) {
        assert_eq!(
            got.segments.len(),
            want.len(),
            "how many segments: got {:?}",
            got.segments
        );
        for (i, (g, w)) in got.segments.iter().zip(want).enumerate() {
            let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
            let points = |s: &Segment| -> Vec<f64> {
                match s {
                    Segment::Move(x, y) | Segment::Line(x, y) => vec![*x, *y],
                    Segment::Curve(a, b, c, d, e, f) => vec![*a, *b, *c, *d, *e, *f],
                }
            };
            assert_eq!(
                std::mem::discriminant(g),
                std::mem::discriminant(w),
                "segment {i} is a different kind of segment: got {g:?}, want {w:?}"
            );
            let (g, w) = (points(g), points(w));
            for (a, b) in g.iter().zip(w.iter()) {
                assert!(
                    near(*a, *b),
                    "segment {i}: got {g:?}, want {w:?}, and {a} is not {b}"
                );
            }
        }
    }

    /// Whether a first segment is within the format's precision of the one given.
    #[track_caller]
    #[allow(clippy::many_single_char_names)]
    fn near(got: Option<&Segment>, want: &Segment) -> bool {
        let Some(got) = got else {
            return false;
        };
        let points = |s: &Segment| match s {
            Segment::Move(x, y) | Segment::Line(x, y) => vec![*x, *y],
            Segment::Curve(a, b, c, d, e, f) => vec![*a, *b, *c, *d, *e, *f],
        };
        let (a, b) = (points(got), points(want));
        std::mem::discriminant(got) == std::mem::discriminant(want)
            && a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// One glyph's rectangle, as the subroutine the bias and CID tests call.
    fn rectangle() -> Vec<u8> {
        Charstring::new()
            .arg(10)
            .arg(20)
            .op(21) // rmoveto
            .arg(90)
            .arg(0)
            .op(5) // rlineto
            .arg(0)
            .arg(60)
            .op(5) // rlineto
            .arg(-90)
            .arg(-60)
            .op(5) // rlineto
            .op(11) // return
            .build()
    }

    /// A second shape, so that calling the wrong Font DICT's subroutine is visible.
    fn triangle() -> Vec<u8> {
        Charstring::new()
            .arg(200)
            .arg(300)
            .op(21)
            .arg(50)
            .arg(0)
            .op(5)
            .arg(0)
            .arg(80)
            .op(5)
            .op(11)
            .build()
    }

    // ── the charstring walk ────────────────────────────────────────────────────

    /// A move and two lines, closed by `endchar`, at hand-computed coordinates.
    ///
    /// Every point is an exact em figure rather than a comparison against a stored outline,
    /// because the point of the test is the arithmetic. The pen starts at (100, 200) and the
    /// last line is the one `endchar` implies through the fill rule — a renderer needs it,
    /// and no operator writes it, so a reader that forgets it fills the glyph with a notch
    /// in it.
    #[test]
    fn a_move_and_two_lines_come_back_as_those_points() {
        let glyph = Charstring::new()
            .arg(100)
            .arg(200)
            .op(21) // rmoveto
            .arg(300)
            .arg(0)
            .op(5) // rlineto
            .arg(0)
            .arg(400)
            .op(5) // rlineto
            .op(14); // endchar

        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                // A thousand glyph units to the em, so 100 units is a tenth of an em.
                Segment::Move(0.1, 0.2),
                Segment::Line(0.4, 0.2),
                Segment::Line(0.4, 0.6),
                Segment::Line(0.1, 0.2),
            ],
        );
    }

    /// `rrcurveto` is relative to the point the pen is at, and its control points are
    /// offsets rather than positions.
    ///
    /// The two numbers that catch it are the first control point and the end: read as
    /// positions rather than offsets, the first control point would be (20, 30) instead of
    /// (30, 40) and the pen would end at (60, 70) instead of (70, 80).
    #[test]
    fn a_curve_is_relative_to_the_pen_and_its_controls_are_offsets() {
        let glyph = Charstring::new()
            .arg(10)
            .arg(10)
            .op(21) // rmoveto to (10, 10)
            .arg(20)
            .arg(30)
            .arg(40)
            .arg(50)
            .arg(60)
            .arg(70)
            .op(8) // rrcurveto
            .op(14);

        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.01, 0.01),
                // Each point is an offset from the one before it: (10,10)+(20,30) is the
                // first control, +（40, 50) the second, and +(60, 70) the end.
                Segment::Curve(0.03, 0.04, 0.07, 0.09, 0.13, 0.16),
                Segment::Line(0.01, 0.01),
            ],
        );
    }

    /// A hint mask is one bit per stem, and its length comes from the stems declared
    /// before it.
    ///
    /// Nine stems are eighteen operands, so the mask is two bytes. A reader that took one
    /// would read the second as the first coordinate of the move that follows, and the move
    /// would land somewhere else entirely — which is the second half of this test.
    #[test]
    fn a_hint_mask_is_one_bit_per_declared_stem() {
        let nine_stems = |mask: &[u8]| {
            let mut glyph = Charstring::new();
            for stem in 0..9i32 {
                glyph = glyph.arg(stem).arg(stem + 1);
            }
            glyph
                .op(1) // hstem
                .op(19) // hintmask
                .raw(mask)
                .arg(100)
                .arg(200)
                .op(21) // rmoveto
                .op(14)
        };

        let data = cff1(&Spec::one(nine_stems(&[0xff, 0x01])));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            near(
                cff.outline(0).expect("a glyph").segments.first(),
                &Segment::Move(0.1, 0.2)
            ),
            "nine stems make a two-byte mask, and the move after it is still the move"
        );

        // The same charstring with one byte of mask where two are due. It must not come out
        // as the same move, because it is not the same charstring.
        let data = cff1(&Spec::one(nine_stems(&[0xff])));
        let cff = Cff::parse(&data, 0).expect("a font");
        let desynchronised = match cff.outline(0) {
            Ok(outline) => !near(outline.segments.first(), &Segment::Move(0.1, 0.2)),
            // Being refused is as good a way of showing it as landing somewhere else.
            Err(_) => true,
        };
        assert!(
            desynchronised,
            "one byte of mask where two are due reads the second byte as an operand"
        );
    }

    /// Every drawing operator reaches the points the specification says it does.
    ///
    /// One font with one glyph per operator, and the expectation written out in ems. The
    /// operators that share a shape with each other — the flexes and the alternating curve
    /// forms — are here because they are the ones where a *sign* or a *five* is easy to
    /// lose, and a lost sign still produces a closed outline.
    #[test]
    fn every_drawing_operator_reaches_the_points_it_should() {
        // hlineto and vlineto alternate, starting with whichever was named.
        let glyph = Charstring::new()
            .arg(100)
            .arg(100)
            .op(21)
            .arg(50)
            .arg(25)
            .arg(75)
            .op(6) // hlineto, then vlineto
            .arg(-25)
            .arg(50)
            .op(7) // vlineto, then hlineto
            .arg(-100)
            .arg(-75)
            .op(6)
            .op(14);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        // Each operator restarts the alternation at whichever axis it names, so three
        // operands to `hlineto` are horizontal, vertical, horizontal — the next operator
        // starts again rather than carrying the alternation over.
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.1, 0.1),
                Segment::Line(0.15, 0.1),    // hlineto 50
                Segment::Line(0.15, 0.125),  // vlineto 25
                Segment::Line(0.225, 0.125), // hlineto 75
                Segment::Line(0.225, 0.1),   // vlineto -25
                Segment::Line(0.275, 0.1),   // hlineto 50
                Segment::Line(0.175, 0.1),   // hlineto -100
                Segment::Line(0.175, 0.025), // vlineto -75
                Segment::Line(0.1, 0.1),
            ],
        );

        // `hflex1`: the first curve is flat and the second closes the gap in y.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40)
            .arg(50)
            .arg(60)
            .arg(70)
            .arg(30)
            .arg(80)
            .escape(36); // hflex1
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (10, 20) then +(30, 40) then +(50, 0): (10,20) (40,60) (90,60)
                Segment::Curve(0.01, 0.02, 0.04, 0.06, 0.09, 0.06),
                // From (90, 60): (60, 0) then +(70, 30) then +(80, -90)
                Segment::Curve(0.15, 0.06, 0.22, 0.09, 0.30, 0.0),
                Segment::Line(0.0, 0.0),
            ],
        );

        // `vvcurveto` with an odd operand count: the first value is the x of the first
        // control point, and no other curve gets one.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(5) // the optional leading dx, which makes the count odd
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40)
            .arg(50)
            .arg(60)
            .arg(70)
            .arg(80)
            .op(26);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (dx1 dya) (dxb dyb) (0 dyc): (5,10) then +(20,30) then +(0,40)
                Segment::Curve(0.005, 0.01, 0.025, 0.04, 0.025, 0.08),
                // The second curve has no dx1, because the first spent the operand.
                Segment::Curve(0.025, 0.13, 0.085, 0.2, 0.085, 0.28),
                Segment::Line(0.0, 0.0),
            ],
        );

        // `rcurveline`: whole curves and then one line, which is 6k + 2 operands. Fourteen
        // of them is two curves and a line; a sixteenth would be one group too many and
        // gets refused rather than half-drawn.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40)
            .arg(50)
            .arg(60)
            .arg(70)
            .arg(80)
            .arg(90)
            .arg(100)
            .arg(110)
            .arg(120)
            .arg(130)
            .arg(140)
            .op(24); // rcurveline
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (10, 20) +(30, 40) +(50, 60): (10,20) (40,60) (90,120)
                Segment::Curve(0.01, 0.02, 0.04, 0.06, 0.09, 0.12),
                Segment::Curve(0.16, 0.2, 0.25, 0.3, 0.36, 0.42),
                // The line is the last two operands, from (360, 420).
                Segment::Line(0.49, 0.56),
                Segment::Line(0.0, 0.0),
            ],
        );

        // The same charstring with one group too many is refused rather than half-drawn: a
        // dropped coordinate is a glyph with a notch in it, and nothing else would say so.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(10)
            .arg(20)
            .arg(20)
            .arg(30)
            .arg(30)
            .arg(40)
            .arg(40)
            .arg(50)
            .arg(50)
            .arg(60)
            .arg(60)
            .arg(70)
            .arg(70)
            .arg(80)
            .arg(80)
            .op(24); // rcurveline
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            cff.outline(0).is_err(),
            "sixteen operands to rcurveline is not 6k + 2"
        );
    }

    /// `hvcurveto` and `vhcurveto` alternate, and the two forms differ only in which axis
    /// the first control point is on.
    ///
    /// This is the one pair of operators where swapping two operands still produces a closed
    /// outline: the four operands are `(dx1 dx2 dy2 dy3)` in one and `(dy1 dx2 dy2 dy3)` in
    /// the other, so reading the last two the wrong way round moves the end of every curve
    /// without anything looking broken. Both forms are here, plus a run of two curves in
    /// `hvcurveto` so that the alternation itself is checked.
    #[test]
    fn the_alternating_curve_operators_read_their_operands_in_each_forms_own_order() {
        // `hvcurveto` with four operands: (dx1 dx2 dy2 dx3), so the curve goes to
        // (dx3, dy3) with dy3 zero unless a fifth operand says otherwise.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40)
            .op(31);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (10, 0) +(20, 30) +(0, 40): (10,0) (30,30) (30,70)
                Segment::Curve(0.01, 0.0, 0.03, 0.03, 0.03, 0.07),
                Segment::Line(0.0, 0.0),
            ],
        );

        // The mirror image: `vhcurveto` puts the zero on the other axis.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40)
            .op(30);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // The mirror image: (0, 10) +(20, 30) +(40, 0): (0,10) (20,40) (60,40)
                Segment::Curve(0.0, 0.01, 0.02, 0.04, 0.06, 0.04),
                Segment::Line(0.0, 0.0),
            ],
        );

        // Two curves in a row: the second starts where the first ended, and alternates.
        let glyph = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(20)
            .arg(30)
            .arg(40) // first curve, horizontal: ends at (30, 70)
            .arg(5) // second curve, vertical: (dy1 dx2 dy2 dy3)
            .arg(6)
            .arg(7)
            .arg(8)
            .op(31);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (10, 0) +(20, 30) +(0, 40): (10,0) (30,30) (30,70)
                Segment::Curve(0.01, 0.0, 0.03, 0.03, 0.03, 0.07),
                // The second curve is a *vertical* one: an `hvcurveto` alternates starting
                // with the axis it names, so (5, 6, 7, 8) reads as (dy1 dx2 dy2 dyc) and
                // runs from (30, 70) to (44, 82) — the fourth operand is the *x* of the end
                // point, and only a fifth would give it a y.
                Segment::Curve(0.03, 0.075, 0.036, 0.082, 0.044, 0.082),
                Segment::Line(0.0, 0.0),
            ],
        );

        // A fifth operand on the *last* curve is the x of its end point in `hvcurveto`, and
        // the y of it in `vhcurveto`.
        let last_horizontal = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(1)
            .arg(2)
            .arg(3)
            .arg(4)
            .arg(5)
            .op(31);
        let data = cff1(&Spec::one(last_horizontal));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (1, 0) +(2, 3) +(5, 4): (1,0) (3,3) (8,7)
                Segment::Curve(0.001, 0.0, 0.003, 0.003, 0.008, 0.007),
                Segment::Line(0.0, 0.0),
            ],
        );
        let last_vertical = Charstring::new()
            .arg(0)
            .arg(0)
            .op(21)
            .arg(1)
            .arg(2)
            .arg(3)
            .arg(4)
            .arg(5)
            .op(30);
        let data = cff1(&Spec::one(last_vertical));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.0, 0.0),
                // (0, 1) +(2, 3) +(4, 5): (0,1) (2,4) (6,9)
                Segment::Curve(0.0, 0.001, 0.002, 0.004, 0.006, 0.009),
                Segment::Line(0.0, 0.0),
            ],
        );
    }

    // ── the subroutine bias ────────────────────────────────────────────────────

    /// The bias is 107, 1131 or 32768, and which one depends on the count.
    ///
    /// Stated at every count that matters rather than at one per range, because the
    /// boundaries are where an off-by-one lives and one sample inside a range cannot see
    /// one.
    #[test]
    fn the_subroutine_bias_is_the_one_the_ranges_name() {
        assert_eq!(Cff::subr_bias(0), 107, "no subroutines at all");
        assert_eq!(Cff::subr_bias(1239), 107, "one below the first boundary");
        assert_eq!(
            Cff::subr_bias(1240),
            1131,
            "1240 subroutines is the first count on the far side of it"
        );
        assert_eq!(Cff::subr_bias(33899), 1131, "one below the second boundary");
        assert_eq!(
            Cff::subr_bias(33900),
            32768,
            "33 900 subroutines is the first count on the far side"
        );
        assert_eq!(Cff::subr_bias(100_000), 32768);
    }

    /// The bias is added to the operand, and at each boundary the right answer reaches a
    /// different subroutine from the wrong one.
    ///
    /// Subroutine zero draws a rectangle and every other subroutine draws nothing, so a bias
    /// one out at either boundary runs an empty subroutine and the glyph comes out with no
    /// outline at all. The counts are 1239, 1240 and 33 900 — the two boundaries and one
    /// either side of the first — so the check is on the number of subroutines the bias was
    /// chosen for rather than on the bias in isolation.
    #[test]
    fn the_bias_reaches_the_right_subroutine_at_both_boundaries() {
        for count in [1239usize, 1240, 33_900] {
            let bias = Cff::subr_bias(count) as i32;
            let mut subrs: Vec<Vec<u8>> = Vec::with_capacity(count);
            for i in 0..count {
                subrs.push(if i == 0 { rectangle() } else { Vec::new() });
            }
            let glyph = Charstring::new().arg(-bias).op(10).op(14);
            let spec = Spec::with_subrs(glyph, subrs);

            let data = cff1(&spec);
            let cff = Cff::parse(&data, 0).expect("a font");
            assert_eq!(
                cff.local_subrs(0).len(),
                count,
                "the font really has the count the bias was chosen for"
            );
            assert!(
                near(
                    cff.outline(0).expect("a glyph").segments.first(),
                    &Segment::Move(0.01, 0.02)
                ),
                "with {count} subroutines the bias is {bias}, so the call must reach \\
                 subroutine zero and nothing else"
            );
        }
    }

    /// Exactly the limit's worth of nesting is allowed, so the limit is a limit and not a
    /// guess one below it.
    #[test]
    fn nesting_exactly_to_the_limit_is_allowed() {
        // `MAX_DEPTH` subroutines, each calling the next, with the last drawing. With 107 of
        // bias, calling subroutine `i` is the operand `i - 107`.
        let mut subrs: Vec<Vec<u8>> = (0..MAX_DEPTH)
            .map(|i| Charstring::chain(i + 1).build())
            .collect();
        if let Some(last) = subrs.last_mut() {
            *last = Charstring::new()
                .arg(10)
                .arg(20)
                .op(21)
                .arg(90)
                .arg(0)
                .op(5)
                .op(11)
                .build();
        }
        let glyph = Charstring::new().arg(-107).op(10).op(14);
        let data = cff1(&Spec::with_subrs(glyph, subrs));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            near(
                cff.outline(0).expect("a glyph").segments.first(),
                &Segment::Move(0.01, 0.02)
            ),
            "{MAX_DEPTH} nested calls reach the bottom of the chain"
        );
    }

    // ── the width ──────────────────────────────────────────────────────────────

    /// A charstring may name its own advance width, and it is written *before* the
    /// coordinates — so it is the bottom of the stack, and taking the wrong end of it moves
    /// every glyph by its own width.
    ///
    /// Both directions are here because both are a way to be wrong. Given three operands to
    /// `rmoveto` the width is the first; given two to `hmoveto` it is *also* the first,
    /// because `hmoveto` takes one coordinate and so expects an even count where `rmoveto`
    /// expects an odd one. Getting that backwards turns the width into the coordinate and
    /// the glyph starts at 500 units instead of 30.
    #[test]
    fn a_charstring_that_names_its_own_width_has_it_taken_off_the_stack() {
        let glyph = Charstring::new()
            .arg(500) // the width
            .arg(100)
            .arg(200)
            .op(21) // rmoveto
            .arg(50)
            .arg(0)
            .op(5)
            .op(14);
        let data = cff1(&Spec::one(glyph).widths(20, 500));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            near(
                cff.outline(0).expect("a glyph").segments.first(),
                &Segment::Move(0.1, 0.2)
            ),
            "the width is discarded and the move starts where the coordinates say"
        );
        assert_eq!(
            cff.advance(0),
            Ok(520),
            "the width is nominalWidthX plus what the charstring wrote"
        );
    }

    /// The even-count case: a width before `hmoveto`, and no width at all.
    #[test]
    fn a_charstring_with_one_coordinate_and_a_width_takes_off_the_first() {
        let glyph = Charstring::new()
            .arg(400) // the width
            .arg(30) // the coordinate
            .op(22) // hmoveto
            .arg(10)
            .arg(10)
            .op(5)
            .op(14);
        let data = cff1(&Spec::one(glyph).widths(20, 500));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            near(
                cff.outline(0).expect("a glyph").segments.first(),
                &Segment::Move(0.03, 0.0)
            ),
            "the coordinate is the second operand, not the first: reading the other end \
             would put the pen at 400 units"
        );
        assert_eq!(cff.advance(0), Ok(420));
    }

    /// A charstring that names no width gets `defaultWidthX`, and its coordinates are all
    /// that is on the stack.
    #[test]
    fn a_charstring_with_no_width_gets_the_default_one() {
        let glyph = Charstring::new()
            .arg(30)
            .op(22) // hmoveto, one operand and so no width
            .arg(10)
            .arg(10)
            .op(5)
            .op(14);
        let data = cff1(&Spec::one(glyph).widths(20, 500));
        let cff = Cff::parse(&data, 0).expect("a font");
        assert!(
            near(
                cff.outline(0).expect("a glyph").segments.first(),
                &Segment::Move(0.03, 0.0)
            ),
            "with no width the one operand is the coordinate"
        );
        assert_eq!(
            cff.advance(0),
            Ok(500),
            "and the width is defaultWidthX, which is what the format says"
        );
    }

    // ── damage ─────────────────────────────────────────────────────────────────

    /// A charstring that leaves values on the stack at `endchar` is refused.
    ///
    /// Drawing the shape up to that point would be drawing a glyph that is not the glyph:
    /// the missing values are operands of an operator that is not there, so whatever the
    /// file meant it is not what came out.
    #[test]
    fn a_charstring_that_leaves_the_stack_odd_at_endchar_is_refused() {
        let glyph = Charstring::new()
            .arg(100)
            .arg(200)
            .op(21) // rmoveto, which also takes any width there is
            .arg(7)
            .arg(9) // two values and no operator to clear them
            .op(14);
        let data = cff1(&Spec::one(glyph));
        let cff = Cff::parse(&data, 0).expect("a font");
        let reason = cff.outline(0).expect_err("a reason, not a glyph");
        assert!(
            reason.contains("2 value") && reason.contains("endchar"),
            "the reason must say how many and where: {reason}"
        );
    }

    /// Fifty values on the stack is past the limit, and the limit is not raised.
    #[test]
    fn a_charstring_that_overruns_the_stack_is_refused() {
        let mut glyph = Charstring::new();
        for i in 0..60 {
            glyph = glyph.arg(i);
        }
        let data = cff1(&Spec::one(glyph.op(14)));
        let cff = Cff::parse(&data, 0).expect("a font");
        let reason = cff.outline(0).expect_err("a reason, not a glyph");
        assert!(
            reason.contains(&MAX_STACK.to_string()),
            "the reason must name the limit it passed: {reason}"
        );
    }

    /// A subroutine that calls itself is damage, and stops at the nesting limit.
    ///
    /// Recursion here is not the font author's problem to have solved; it is a loop with
    /// nothing in it that ends. The reason matters as much as the refusal, because a silent
    /// stop would be a glyph that happened to come out plausible.
    #[test]
    fn a_charstring_that_recurses_past_the_depth_limit_is_refused() {
        // Subroutine zero calls subroutine zero, which is the bias of 107 cancelling out.
        let calls_itself = Charstring::new().arg(-107).op(10).op(11).build();
        let spec = Spec::with_subrs(
            Charstring::new().arg(-107).op(10).op(14),
            vec![calls_itself],
        );
        let data = cff1(&spec);
        let cff = Cff::parse(&data, 0).expect("a font");
        let reason = cff.outline(0).expect_err("a reason, not a glyph");
        assert!(
            reason.contains(&MAX_DEPTH.to_string()),
            "the reason must name the limit it passed: {reason}"
        );
    }

    /// A font whose Top DICT says its charstrings are Type 1 is refused, because Type 1
    /// charstrings are a different language and running them as Type 2 would draw a wrong
    /// shape rather than no shape.
    #[test]
    fn a_font_that_says_its_charstrings_are_type_1_is_refused() {
        let mut spec = Spec::one(Charstring::new().arg(100).arg(200).op(21).op(14));
        spec.charstring_type = Some(1);
        let data = cff1(&spec);
        let reason = Cff::parse(&data, 0).expect_err("a reason, not a font");
        assert!(
            reason.contains("Type 1"),
            "the reason must say which dialect it is refusing and why: {reason}"
        );

        // The same font saying 2 is the format's default and is read.
        let mut spec = Spec::one(Charstring::new().arg(100).arg(200).op(21).op(14));
        spec.charstring_type = Some(2);
        let data = cff1(&spec);
        let cff = Cff::parse(&data, 0).expect("a Type 2 font");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[Segment::Move(0.1, 0.2)],
        );
    }

    // ── CID fonts ──────────────────────────────────────────────────────────────

    /// A CID-keyed font gives each glyph its own Private DICT, so which subroutines a
    /// glyph may call is a property of the glyph.
    ///
    /// Two Font DICTs, two subroutines that draw different shapes, and two glyphs that call
    /// their own FD's subroutine zero with the *same* operand. A reader that ignored
    /// `/FDSelect` would give both glyphs FD 0's rectangle; one that read it and then used
    /// the wrong FD would give both triangles. Only reading both gets this.
    #[test]
    fn a_cid_font_gives_each_glyph_its_own_subroutines_and_widths() {
        let call = Charstring::new().arg(-107).op(10).op(14);
        let spec = CidSpec {
            glyphs: vec![call.clone().build(), call.build()],
            fds: vec![(10, 100, vec![rectangle()]), (700, 800, vec![triangle()])],
            charset: None,
        };
        let data = cff1_cid(&spec);
        let cff = Cff::parse(&data, 0).expect("a CID font");

        assert!(cff.is_cid_keyed(), "ROS is what makes a font CID-keyed");
        assert_eq!(cff.num_glyphs(), 2);
        assert!(
            near(
                cff.outline(0).expect("the first glyph").segments.first(),
                &Segment::Move(0.01, 0.02)
            ),
            "the first glyph uses the first FD's rectangle"
        );
        assert!(
            near(
                cff.outline(1).expect("the second glyph").segments.first(),
                &Segment::Move(0.2, 0.3)
            ),
            "the second uses the second FD's triangle, from its own subroutines"
        );
        assert_eq!(
            cff.widths(0),
            (10.0, 100.0),
            "and each FD's widths are the glyph's own"
        );
        assert_eq!(cff.widths(1), (700.0, 800.0));
    }

    /// A CID font whose charset is not the identity maps identifiers to glyph numbers
    /// through something this does not read, and is refused rather than drawn wrong.
    #[test]
    fn a_cid_font_whose_charset_is_not_the_identity_is_refused_with_a_reason() {
        let glyph = Charstring::new().op(14);
        let mut spec = CidSpec {
            glyphs: vec![glyph.clone().build(), glyph.build()],
            fds: vec![(0, 500, Vec::new())],
            charset: None,
        };
        // An identity charset except for the last glyph, which says it is CID 7.
        let identity = cff1_cid(&spec);
        let cff = Cff::parse(&identity, 0).expect("an identity charset is readable");
        assert_eq!(cff.num_glyphs(), 2);

        spec.charset = Some({
            let mut c = vec![0u8];
            c.extend_from_slice(&9u16.to_be_bytes());
            c
        });
        let other = cff1_cid(&spec);
        let reason = Cff::parse(&other, 0).expect_err("a reason, not a font");
        assert!(
            reason.contains("charset") && reason.contains("identity"),
            "the reason must say what is wrong with it, not merely that it is wrong: {reason}"
        );
    }

    // ── CFF 2 ──────────────────────────────────────────────────────────────────

    /// An item variation store with one record of one region, which is all a `blend` needs
    /// to know how many deltas to eat.
    fn one_region_store() -> Vec<u8> {
        let mut store = vec![0u8; 20];
        store[2..4].copy_from_slice(&1u16.to_be_bytes()); // format
        store[8..10].copy_from_slice(&1u16.to_be_bytes()); // one record
        store[10..14].copy_from_slice(&14u32.to_be_bytes()); // the record is 14 bytes in
        store[14..16].copy_from_slice(&1u16.to_be_bytes()); // itemCount
        store[16..18].copy_from_slice(&0u16.to_be_bytes()); // shortDeltaCount
        store[18..20].copy_from_slice(&1u16.to_be_bytes()); // regionIndexCount
        store
    }

    /// A CFF 2 font: a five-byte header, the Top DICT, the global subroutines, and then
    /// the tables the Top DICT names.
    fn cff2(glyphs: &[Vec<u8>], store: &[u8]) -> Vec<u8> {
        let charstrings = indexed(glyphs, 4);
        let gsubrs = indexed(&[], 4);
        let charstrings_at = 5 + 8 + gsubrs.len();
        let store_at = charstrings_at + charstrings.len();
        let mut top = Vec::new();
        top.extend(fixed(charstrings_at));
        top.push(17); // CharStrings
        top.extend(fixed(store_at));
        top.push(24); // VarStore
        assert_eq!(
            top.len(),
            8,
            "the Top DICT length is what the layout was sized for"
        );

        let mut out = vec![2u8, 0, 5];
        out.extend_from_slice(&u16::try_from(top.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&top);
        out.extend_from_slice(&gsubrs);
        out.extend_from_slice(&charstrings);
        out.extend_from_slice(store);
        out
    }

    /// `vsindex` and `blend` are executed, and with no region scalars to add a blend is
    /// its default value — which is the default instance of a variable font, and is what a
    /// PDF asks for.
    ///
    /// One region, so two blended operands are two defaults and two deltas and a count of
    /// two. The defaults are kept and the deltas dropped, and the move that follows says so:
    /// it lands at (10, 20) and not at (40, 60), which is what adding both deltas would
    /// give.
    #[test]
    fn a_blend_is_its_default_instance_and_has_no_width() {
        assert_eq!(
            Cff::subr_bias(33_900),
            32768,
            "the bias is the same in CFF 2 as in CFF 1"
        );
        let glyph = Charstring::new()
            .arg(0)
            .op(15) // vsindex
            .arg(10) // default
            .arg(20) // default
            .arg(30) // delta
            .arg(40) // delta
            .arg(2) // two blends
            .op(16) // blend, leaving its two defaults
            .op(21) // rmoveto
            .arg(90)
            .arg(0)
            .op(5); // rlineto
        let data = cff2(&[glyph.build()], &one_region_store());
        let cff = Cff::parse(&data, 0).expect("a CFF2 font");

        assert!(cff.is_cff2(), "the version byte says so");
        assert_outline(
            &cff.outline(0).expect("a glyph"),
            &[
                Segment::Move(0.01, 0.02),
                Segment::Line(0.1, 0.02),
                Segment::Line(0.01, 0.02),
            ],
        );
        assert!(
            cff.advance(0).is_err(),
            "a CFF2 glyph has no width at all, and reporting one would be wrong"
        );
    }

    /// `vsindex` naming a record the store does not have is a refusal, because the number of
    /// deltas a blend eats depends on it and a guess would swallow the operator after.
    #[test]
    fn a_vsindex_naming_a_record_that_is_not_there_is_refused() {
        let glyph = Charstring::new().arg(1).op(15).arg(10).arg(0).op(16);
        let data = cff2(&[glyph.build()], &one_region_store());
        let cff = Cff::parse(&data, 0).expect("a CFF2 font");
        let reason = cff.outline(0).expect_err("a reason, not a glyph");
        assert!(
            reason.contains("variation record"),
            "the reason must say what could not be worked out: {reason}"
        );
    }

    // ── a program that is not a CFF ────────────────────────────────────────────

    /// Bytes from the internet are not CFF, and must not panic.
    ///
    /// A PDF can carry any `/FontFile3` at all, and the whole of the tolerance for that
    /// lives here. A panic on a truncated font is a crash on a file a user opened, which is
    /// the one outcome this crate exists to prevent, so every one of these has to come back
    /// as a refusal rather than as an answer.
    #[test]
    fn a_font_that_is_not_a_cff_is_refused_rather_than_a_panic() {
        assert!(crate::outline::from_cff(&[], 0).is_none());
        assert!(
            crate::outline::from_cff(b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n%%EOF\n", 0).is_none(),
            "a PDF is not a font"
        );
        // A TrueType program, which is the other thing a font stream can be.
        let mut true_type = vec![0x00u8, 0x01, 0x00, 0x00];
        true_type.resize(64, 0);
        assert!(crate::outline::from_cff(&true_type, 0).is_none());

        // A real font cut short at every length, which is what a truncated download is.
        let data = cff1(&Spec::one(
            Charstring::new().arg(100).arg(200).op(21).op(14),
        ));
        for cut in 0..data.len() {
            let _ = Cff::parse(&data[..cut], 0);
            let _ = crate::outline::from_cff(&data[..cut], 0);
        }
        // Every byte of it replaced by one of a few values that mean something elsewhere,
        // since one mutation is not the space of broken inputs.
        for byte in [0x00u8, 0x01, 0x0c, 0x1c, 0x7f, 0xff] {
            let broken = vec![byte; data.len()];
            let _ = Cff::parse(&broken, 0);
            let _ = crate::outline::from_cff(&broken, 0);
        }
        // One byte changed at a time, which is a bigger space and the one real damage
        // arrives in.
        for at in 0..data.len() {
            for byte in [0x00u8, 0x03, 0xff] {
                let mut broken = data.clone();
                broken[at] = byte;
                let _ = Cff::parse(&broken, 0);
                let _ = crate::outline::from_cff(&broken, 0);
            }
        }
        // A header claiming a version that is neither 1 nor 2.
        let mut wrong = data.clone();
        wrong[0] = 9;
        assert!(Cff::parse(&wrong, 0).is_err());
    }

    // ── the encodings ──────────────────────────────────────────────────────────

    /// Every operand encoding decodes to the number it is supposed to.
    ///
    /// Written as raw bytes rather than through the builder above, because the point is the
    /// four encodings themselves and the builder would only ever produce one of them.
    #[test]
    fn every_operand_encoding_decodes() {
        let cases: [(&[u8], f32); 9] = [
            (&[32], -107.0),
            (&[139], 0.0),
            (&[246], 107.0),
            (&[247, 0], 108.0),
            (&[250, 255], 1131.0),
            (&[251, 0], -108.0),
            (&[254, 255], -1131.0),
            (&[28, 0xff, 0x9c], -100.0),
            (&[255, 0x01, 0x00, 0x00, 0x00], 256.0),
        ];
        for (bytes, want) in cases {
            let spanned = token_at(bytes, 0).expect("a token");
            assert_eq!(spanned.token, Token::Number(want), "{bytes:?} is {want}");
            assert_eq!(spanned.len, bytes.len(), "{bytes:?} takes that many bytes");
        }
        // Bytes 29 and 30 are a DICT's five-byte integer and its real number, and in a
        // charstring they are `callgsubr` and `vhcurveto`. Reading them as operand
        // encodings refuses most of a real font, which is how this was found.
        assert_eq!(token_at(&[29], 0).expect("a token").token, Token::Op(29));
        assert_eq!(token_at(&[30], 0).expect("a token").token, Token::Op(30));
        // In a DICT they are numbers: a five-byte integer and a binary-coded decimal, each
        // followed by the operator that names what it means.
        let five_byte = Dict::parse(&[29, 0, 0, 0, 1, 0]).expect("a DICT");
        assert_eq!(
            five_byte.number(0),
            Some(1.0),
            "b0 = 29 is a 32-bit integer there"
        );
        let real = Dict::parse(&[30, 0x1a, 0x2f, 0]).expect("a DICT");
        assert_eq!(real.number(0), Some(1.2), "b0 = 30 is a real there");
    }

    /// A DICT real number is a decimal expression in nibbles, and the terminator ends it.
    #[test]
    fn a_dict_real_number_is_a_decimal_expression() {
        // `-2.25`, as the specification writes it.
        assert_eq!(read_real(&[0xe2, 0xa2, 0x5f], &mut 0), Some(-2.25));
        // A terminator in the low nibble ends the number at the end of that byte.
        assert_eq!(read_real(&[0x30, 0xf0], &mut 0), Some(30.0));
        // A terminator in the *high* nibble means the number is over, and the specification
        // requires a padding nibble to follow it so the byte count stays whole.
        assert_eq!(read_real(&[0x03, 0xf0], &mut 0), Some(3.0));
        assert_eq!(read_real(&[0x00, 0xff], &mut 0), Some(0.0));
        // A reserved nibble is not part of any number the specification defines.
        assert_eq!(read_real(&[0xd1, 0xf0], &mut 0), None);
        // A real far longer than any of them needs is refused rather than parsed.
        let long = [0x11u8; MAX_REAL_NIBBLES + 4];
        assert_eq!(read_real(&long, &mut 0), None);
    }

    /// The font matrix is the scale from glyph units to ems, and its reciprocal is the
    /// grid.
    ///
    /// A bare CFF program has no `head` table to say so, so the matrix is the only answer —
    /// and a font whose glyph units are not a thousand to the em would come out a thousand
    /// times its stated size with the default.
    #[test]
    fn the_em_comes_from_the_font_matrix_when_there_is_no_head_table() {
        assert_eq!(units_from_matrix(&DEFAULT_FONT_MATRIX), 1000);
        assert_eq!(
            units_from_matrix(&[0.0005, 0.0, 0.0, 0.0005, 0.0, 0.0]),
            2000
        );
        assert_eq!(units_from_matrix(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]), 1000);
    }
}
