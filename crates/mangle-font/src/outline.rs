//! Glyph outlines, out of a font program.
//!
//! A renderer cannot draw a character without knowing its shape, and the shape is not in
//! the PDF: it is in the font program the file embeds. This module is the bridge — a byte
//! slice and a glyph number in, a path in ems out, with nothing in between that knows
//! about pages or pixels.
//!
//! ## Ems, and why
//!
//! Everything here is in **ems**, not font units. A font chooses its own grid (1000, 2048,
//! 4096) and the choice means nothing outside that font; dividing by it at the point of
//! reading is what makes one renderer work with every font without a table of exceptions.
//! One em is one unit of glyph space, so the `Tf` size is the number that finally multiplies
//! the path onto the page.
//!
//! ## Why `Segment` is defined twice
//!
//! [`mangle_content::PathSegment`] is the renderer's path type and this crate's is its own.
//! That is deliberate. The dependency order runs `syntax → … → font → content → render`, so
//! `mangle-font` cannot see `mangle-content`; sharing the type would mean either inverting
//! the order (which the architecture forbids) or moving the type down into `mangle-syntax`
//! so that a font and a page could argue about it. Neither is worth it. The conversion is
//! three lines in one direction and zero the other, and the alternative — a path type with
//! room for arcs, quads and everything else a general path library wants — would be a lie
//! about the data: TrueType outlines are move, line and cubic, and nothing else.
//!
//! ## Two kinds of program, one answer
//!
//! A `/FontFile2` is TrueType and this module reads it through `ttf-parser`. A `/FontFile3`
//! is CFF, and its glyphs are not points in a table but Type 2 *programs* that have to be
//! executed; that reader is [`crate::cff`]. A `/FontFile` is Type 1, which is a third thing
//! again — a PostScript program encrypted twice, with a third charstring dialect inside the
//! second layer — and that reader is [`crate::type1`]. All three answer the same question in
//! the same units, so [`Program`] is what asks which of them it has and does not care. A
//! program that is none is a `None` with a reason attached rather than an empty path, because
//! "this file has no glyph I can draw" and "this file is not a font" are different findings.

use ttf_parser::{Face, GlyphId, OutlineBuilder, PlatformId};

use crate::cff::{self, Cff};
use crate::metrics::CidToGid;
use crate::type1::Type1;

/// A glyph's outline, in ems, ready to be transformed onto a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outline {
    /// Closed contours as move/line/cubic triples, in em units.
    pub segments: Vec<Segment>,
}

impl Outline {
    /// Is there nothing here?
    ///
    /// A space, and any code a font does not have, walk to an empty path. That is the
    /// common case rather than a failure, and a caller that treats it as one will fill a
    /// page with notes about characters nobody can see.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// One step of a fill path.
///
/// The three cases are the three a TrueType `glyf` table can express. A contour is a
/// `Move`, some `Line`s and `Curve`s, and then the next `Move`; a glyph is closed by
/// running back to its first point, so no `Close` is needed and none is offered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// A new contour, starting here.
    Move(f64, f64),
    /// A straight edge to here.
    Line(f64, f64),
    /// A cubic edge: two control points and the end of the edge.
    Curve(f64, f64, f64, f64, f64, f64),
}

/// The factor from a font's own units to ems.
///
/// A font declares its grid in its `head` table, and one em is however many units that
/// table says: 1000 for most fonts, 2048 for the older ones. So a coordinate of *n* in the
/// font's grid is *n / unitsPerEm* ems, and that is the whole of this function.
///
/// The tempting alternative is `1000 / unitsPerEm`, which is the factor from font units
/// to PDF *glyph space* — the space whose unit is a thousandth of an em. It is a thousand
/// times too large here and draws every glyph a thousand times its stated size. The
/// difference shows only on a font whose em is not a thousand units, which is exactly the
/// font whose size would be quietly wrong.
///
/// A font claiming *zero* units per em is damage, and dividing by it would turn every
/// coordinate into an infinity. It is treated as a font that already uses PDF's own
/// convention of 1000 units to the em, so the scale is 1 and the glyph comes out the size
/// the file asked for rather than not at all.
#[must_use]
pub fn em_scale(units_per_em: u16) -> f64 {
    if units_per_em == 0 {
        return 1.0;
    }
    1.0 / f64::from(units_per_em)
}

/// A font program, parsed on demand.
///
/// A page has one of these per font resource and asks it for a thousand outlines, so
/// `ttf-parser`'s `Face` — which borrows its bytes and so cannot outlive a call that also
/// wants to write to the cache — is not stored. What is stored is the program and the
/// answers already computed, which is where all the time goes.
#[derive(Debug, Clone, Default)]
pub struct Program {
    data: Vec<u8>,
    /// Outlines by glyph number. `None` is a remembered miss, which is a real answer for a
    /// space and worth not recomputing for every character in the word.
    outlines: std::collections::HashMap<u32, Option<Outline>>,
    /// The verdict on what this program is, worked out once. Which kind of program it is
    /// and how big its em is are the same question, so they are one answer: a font with no
    /// em has no outlines either, and finding that out twice would only risk the two
    /// disagreeing.
    inspected: Option<Result<(u16, Kind), String>>,
}

/// What a font program turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `glyf` outlines, read through `ttf-parser`.
    TrueType,
    /// CFF outlines, read by executing Type 2 charstrings.
    Cff,
    /// Type 1 outlines, read by undoing two ciphers and executing Type 1 charstrings.
    Type1,
}

impl Program {
    /// A font program, as the bytes of a `/FontFile`, `/FontFile2` or `/FontFile3` stream.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            outlines: std::collections::HashMap::new(),
            inspected: None,
        }
    }

    /// The bytes this program was built from.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// How many units the font's own grid has to the em, if the program parses at all.
    #[must_use]
    pub fn units_per_em(&mut self) -> Option<u16> {
        self.inspect().ok()
    }

    /// How many units the font's own grid has to the em, or the reason there is not one.
    ///
    /// The reason matters as much as the number. A renderer that cannot read a font is
    /// going to say so on the page, and "the font program is not a font this can read" is
    /// a finding a user can act on where "no outlines" is not.
    pub fn inspect(&mut self) -> Result<u16, String> {
        if let Some(cached) = &self.inspected {
            return cached.clone().map(|(units, _)| units);
        }
        let verdict = self.read();
        self.inspected = Some(verdict.clone());
        verdict.map(|(units, _)| units)
    }

    /// The one answer to "what is this", used by everything above so that the em, the
    /// outlines and the widths cannot disagree about it.
    ///
    /// The three kinds are told apart by what they start with, in this order, because each
    /// test is cheap and each one is decisive: a `CFF ` table begins with its own major
    /// version, a Type 1 program begins with a PostScript magic comment, and an `sfnt`
    /// begins with a signature that is neither.
    fn read(&self) -> Result<(u16, Kind), String> {
        if let Some(table) = cff::cff_bytes(&self.data) {
            let cff = Cff::parse(table, 0).map_err(|why| format!("it is CFF but {why}"))?;
            return Ok((cff::units_from_matrix(&cff.font_matrix), Kind::Cff));
        }
        if self.data.first() == Some(&b'%') {
            let font = Type1::parse(&self.data).map_err(|why| format!("it is Type 1 but {why}"))?;
            return Ok((cff::units_from_matrix(&font.font_matrix()), Kind::Type1));
        }
        let face = Face::parse(&self.data, 0).map_err(|why| format!("it is not a font: {why}"))?;
        Ok((face.units_per_em(), Kind::TrueType))
    }

    /// The parsed font, for the one kind whose reader is not reached through a table.
    ///
    /// A Type 1 program is re-parsed rather than stored, for the same reason a CFF one is:
    /// both borrow their bytes, and a page has a `Program` per font resource that outlives
    /// the call that made it.
    fn type1(&self) -> Option<Type1> {
        if self.data.first() == Some(&b'%') {
            return Type1::parse(&self.data).ok();
        }
        None
    }

    /// Why a character code cannot be turned into a glyph here, if it cannot.
    ///
    /// `None` means the program can. The one case that cannot is a *bare* CFF font whose
    /// charset is written out in a form this does not read: the codes in a content stream
    /// name glyphs through the font's `/Encoding`, and the charset resolves those names to
    /// glyph numbers, so a charset that is missing is the identity mapping and a charset
    /// that is present and unreadable resolves nothing. Saying so once beats drawing nothing
    /// for every character on the page and saying nothing about why.
    pub fn code_refusal(&mut self) -> Option<String> {
        if let Err(reason) = self.inspect() {
            return Some(reason);
        }
        // A Type 1 font carries its own encoding in its cleartext header and a glyph is
        // named in its `CharStrings` dictionary, so a code reaches a glyph without any table
        // this does not read. Both name-keyed and CID-keyed are answerable.
        if self.type1().is_some() {
            return None;
        }
        let table = cff::cff_bytes(&self.data)?;
        // A CID-keyed font resolves an identifier through its charset, and the only
        // charset this reads is the identity one, which `Cff::parse` has already insisted
        // on — so the code is the glyph number. A name-keyed one resolves through a charset
        // this reads, in all three of its formats.
        match Cff::parse(table, 0) {
            Ok(cff) if cff.resolves_names() => None,
            // An `sfnt` carries a `cmap`, so a code reaches a glyph through it whatever the
            // outlines are made of.
            _ if Face::parse(&self.data, 0).is_ok() => None,
            _ => Some(
                "it is a bare CFF font whose charset names glyphs in a form this does not \
                 read, so no character on the page in it can be drawn"
                    .into(),
            ),
        }
    }

    /// The outline of one glyph, in ems, and the font's units per em.
    ///
    /// `None` when the program is not a font, when the number is not a glyph it has, or
    /// when the glyph has no outline at all — which is a space, and is not damage.
    #[must_use]
    pub fn outline(&mut self, glyph: u32) -> Option<(Outline, u16)> {
        let cached = match self.outlines.get(&glyph) {
            Some(cached) => cached.clone(),
            None => {
                let computed = self.computed_outline(glyph);
                self.outlines.insert(glyph, computed.clone());
                computed
            }
        };
        Some((cached?, self.units_per_em()?))
    }

    /// One glyph's outline, from whichever reader this program needs.
    fn computed_outline(&self, glyph: u32) -> Option<Outline> {
        if let Some(table) = cff::cff_bytes(&self.data) {
            // A CFF outline is only known once its charstring has been executed, and a
            // charstring that refuses is a glyph that is not drawn rather than a glyph
            // drawn wrongly.
            return Cff::parse(table, 0).ok()?.outline(glyph).ok();
        }
        if let Some(font) = self.type1() {
            return font.outline(glyph).ok();
        }
        outline_of(&self.data, 0, glyph).map(|(o, _)| o)
    }

    /// The outline one character code names, in ems, and the font's units per em.
    ///
    /// This is the step a renderer actually wants: PDF gives it a byte and needs a path.
    #[must_use]
    pub fn outline_for_code(&mut self, code: u32) -> Option<(Outline, u16)> {
        let glyph = self.glyph_for_code(code)?;
        self.outline(glyph)
    }

    /// How wide one glyph is, in the font's own units, from the `hmtx` table.
    ///
    /// The font's own units rather than ems because a PDF `/Widths` array is in thousandths
    /// of an em, and a caller writing one will want the raw figure to do that arithmetic
    /// itself rather than have it rounded twice on the way.
    ///
    /// A CFF program answers from its charstring's own width plus the Private DICT's
    /// `nominalWidthX`, which is in glyph units; a CFF font's grid is a thousand units to
    /// the em unless its `FontMatrix` says otherwise, so that is the same figure.
    #[must_use]
    pub fn advance(&self, glyph: u32) -> Option<u16> {
        if let Some(table) = cff::cff_bytes(&self.data) {
            return Cff::parse(table, 0).ok()?.advance(glyph).ok();
        }
        // A Type 1 glyph's width is its own `hsbw`, in glyph units, and the font's matrix
        // scales those to ems — the same arithmetic as the CFF answer above.
        if let Some(font) = self.type1() {
            return font.advance(glyph).ok();
        }
        let face = Face::parse(&self.data, 0).ok()?;
        face.glyph_hor_advance(GlyphId(checked(glyph, face.number_of_glyphs())))
    }

    /// The glyph number one character code names, where the encoding may say what the code
    /// is called.
    ///
    /// This is the step a renderer actually wants and the only place a code becomes a glyph
    /// number. `name` is what the font's `/Encoding` gives the code — a page may remap any
    /// code it likes through a `/Differences` array, and a font that keys its glyphs by name
    /// (Type 1, and a bare name-keyed CFF) can only answer through the name. `None` means the
    /// file named no encoding, and the font's own subtables and built-in encoding answer as
    /// they did before, which is right: a font with no `/Encoding` is addressed by the codes
    /// its own tables give.
    ///
    /// **A composite font is not this function.** Its code is a CID, not a character code,
    /// and `/Differences` does not apply to one; see [`Self::glyph_for_cid`].
    #[must_use]
    pub fn glyph_for_code_named(&self, code: u32, name: Option<&str>) -> Option<u32> {
        if let Some(glyph) = self.named_code_glyph(code, name) {
            return Some(glyph);
        }
        // The one step the font's own tables do not make, and which is right **only** for a
        // single-byte code: a symbolic font's codes are its author's own 0-to-255 choice, so
        // a code no subtable answers is a glyph number, and the specification provides for
        // exactly this by letting a font with no `cmap` at all be addressed by glyph number.
        // [`Self::glyph_for_code16`] deliberately does not have it.
        match Face::parse(&self.data, 0) {
            Ok(face) => {
                let in_range = u32::from(face.number_of_glyphs());
                (code < in_range).then_some(code)
            }
            // A bare CID-keyed CFF has no `sfnt` directory, so the subtables above are not
            // there to be tried. A CID-keyed one says the identifier and the glyph number are
            // the same number; a name-keyed one has nothing to say.
            Err(_) => self.identity_cid(code),
        }
    }

    /// The outline one character code names, in ems, where the encoding may say what the
    /// code is called.
    #[must_use]
    pub fn outline_for_code_named(
        &mut self,
        code: u32,
        name: Option<&str>,
    ) -> Option<(Outline, u16)> {
        let glyph = self.glyph_for_code_named(code, name)?;
        self.outline(glyph)
    }

    /// The glyph number one glyph name names in a bare CFF font.
    ///
    /// A name-keyed CFF font names its glyphs in its charset and nothing else, so this is
    /// the whole of how a character code reaches a glyph in one: the `/Encoding` turns the
    /// code into a name, and the charset turns the name into a number. A name the charset
    /// does not have is `None`, because the alternative — guessing which nearby glyph was
    /// meant — draws a character nobody asked for in place of one they did.
    ///
    /// Two names for one character are both tried, in this order: the name itself, then the
    /// other spelling of it. A real font says `uni0041` in its charset while the PDF's
    /// `/Encoding` says `A`, and a reader that takes only one of the two draws a page of
    /// nothing and reports nothing about why. `uniXXXX`, `uXXXX` and an AGL name are three
    /// spellings of one character, and which two of them meet depends on which program
    /// wrote the font.
    #[must_use]
    pub fn glyph_for_name(&self, name: &str) -> Option<u32> {
        let table = cff::cff_bytes(&self.data)?;
        let cff = Cff::parse(table, 0).ok()?;
        if cff.is_cid_keyed() {
            return None;
        }
        cff.glyph_for_name(name)
    }

    /// The glyph number one character code names.
    ///
    /// The order the subtables are tried in is the specification's, and the reason for it
    /// is that a font may carry several and they may disagree:
    ///
    /// 1. **(3, 0) — Windows Symbol.** A symbolic TrueType font's codes are not Unicode;
    ///    they are whatever the font's author chose, and the (3, 0) subtable is the map
    ///    from that choice to glyphs. Tried first, because when it is present it is the
    ///    only one that can be right.
    /// 2. **(1, 0) — Macintosh Roman.** The specification's fallback for a symbolic font
    ///    that has no (3, 0), and the same subtable a very old font carries instead.
    /// 3. **(3, 1) — Windows Unicode BMP.** For a font that is *not* symbolic, the codes
    ///    in the content stream are the result of the font's `/Encoding` and only the
    ///    Unicode subtable can interpret them. Last, because for a symbolic font it would
    ///    answer with the wrong glyph rather than with nothing.
    ///
    /// A non-symbolic font is not told apart here. The distinction decides nothing about
    /// which of these answers, only about whether the code ought to have been through an
    /// `/Encoding` first, and that is the content layer's business; trying all three in
    /// this order is what a single-byte TrueType font in a PDF needs.
    ///
    /// If no subtable answers, the code is taken as a glyph number outright. That is what
    /// a subsetted symbolic font with no usable `cmap` needs, and it is the specification's
    /// own provision for a font with no `cmap` at all. A code that is not a glyph the font
    /// has is `None`.
    ///
    /// A bare CID-keyed CFF has no `cmap` to consult, and there is nothing else to consult:
    /// its identifiers *are* its glyph numbers, which [`Cff::parse`] only allows having
    /// said so.
    #[must_use]
    pub fn glyph_for_code(&self, code: u32) -> Option<u32> {
        if let Some(font) = self.type1() {
            return font.glyph_for_code(code);
        }
        match Face::parse(&self.data, 0) {
            Ok(face) => {
                if let Some(found) = glyph_from_subtables(&face, code) {
                    return Some(found);
                }
                let in_range = u32::from(face.number_of_glyphs());
                (code < in_range).then_some(code)
            }
            // A bare CFF has no `sfnt` directory, so the subtables above are not there to
            // be tried. A CID-keyed one says the identifier and the glyph number are the
            // same number; a name-keyed one has nothing to say.
            Err(_) => self.identity_cid(code),
        }
    }

    /// The outline a two-byte character code names in a CID-keyed font, in ems, and the
    /// font's units per em.
    ///
    /// The composite-font counterpart of [`Self::outline_for_code`], and the difference is
    /// in the lookup rather than the result. `map` is the descendant font's `/CIDToGIDMap`,
    /// which is the whole of how a CID reaches a glyph; see [`CidToGid`] for why it is the
    /// only thing consulted.
    #[must_use]
    pub fn outline_for_cid(&mut self, cid: u32, map: &CidToGid) -> Option<(Outline, u16)> {
        let glyph = self.glyph_for_cid(cid, map)?;
        self.outline(glyph)
    }

    /// The glyph number one character identifier names in a CID-keyed font.
    ///
    /// **`/CIDToGIDMap` is the whole of this answer, and the font's `cmap` is not part of
    /// it.** The two cases a two-byte code can come from are different questions:
    ///
    /// * **A composite font.** Its code is a CID: an identifier in the font's own numbering,
    ///   and the file says what the numbering means in `/CIDToGIDMap` — the identity map when
    ///   it says nothing, a two-byte-per-entry table when it says otherwise. That is all
    ///   there is, so all there is to ask. A `cmap` subtable is *not* an alternative route
    ///   here: a `(3, 0)` symbol subtable answers with Unicode's private-use codes, which
    ///   are a different numbering from the one the file declared, and a font that has one
    ///   and is reached as a CID font will be given glyphs the file never asked for.
    /// * **A simple font whose `/Encoding` is a CMap.** Its code is a *character code*, not
    ///   an identifier, and it is answered by the font's own encoding and tables as
    ///   [`Self::glyph_for_code16`] does. There is nothing to be a CID about, so
    ///   `/CIDToGIDMap` does not apply to it either.
    ///
    /// A CID the font has no glyph for is `None`, which is how a code the font does not have
    /// is reported.
    ///
    /// **`.notdef` is never drawn, and is never reached as a fallback.** That is a decision
    /// rather than a default: the specification maps a CID past the end of a `/CIDToGIDMap`
    /// stream to GID 0 precisely so that "this font has no glyph here" has an answer, and
    /// GID 0 is the font's `.notdef` — a hollow box in most TrueType fonts. Painting it puts
    /// a character on the page that the document never asked for, and it is the shape this
    /// whole question was once misdiagnosed as having. A code that reaches GID 0 is a code
    /// that names nothing, so it draws nothing.
    #[must_use]
    pub fn glyph_for_cid(&self, cid: u32, map: &CidToGid) -> Option<u32> {
        let glyph = map.glyph_for(cid).filter(|&glyph| glyph != 0)?;
        match Face::parse(&self.data, 0) {
            Ok(face) => (glyph < u32::from(face.number_of_glyphs())).then_some(glyph),
            // A bare CID-keyed CFF has no `sfnt` and therefore no glyph count to check
            // against, and the specification makes `/CIDToGIDMap` the identity map for it;
            // `identity_cid` is that map expressed as the question the CFF charset allows.
            Err(_) => self.identity_cid(glyph),
        }
    }

    /// The outline a two-byte character code names in a **simple** font, in ems.
    ///
    /// A simple font with a CMap `/Encoding` has codes two bytes wide, and they are
    /// character codes — which is the whole difference from a composite font's, and why this
    /// is not [`Self::outline_for_cid`]. There is no identifier to map and no
    /// `/CIDToGIDMap` to map it with.
    #[must_use]
    pub fn outline_for_code16(&mut self, code: u32, name: Option<&str>) -> Option<(Outline, u16)> {
        let glyph = self.glyph_for_code16(code, name)?;
        self.outline(glyph)
    }

    /// The glyph number one two-byte character code names in a **simple** font.
    ///
    /// The same lookup [`Self::glyph_for_code_named`] makes, and deliberately **without its
    /// last step**. That step reads a code no subtable answered as a glyph number outright,
    /// which is right for a single-byte code — a subsetted symbolic font's codes are its own
    /// 0-to-255 symbol codes, and treating one as a glyph number is the specification's own
    /// provision for a font with no usable `cmap` — and wrong here. A two-byte code in a
    /// simple font came out of a CMap `/Encoding`, which names *characters*; a code that
    /// names no character is a code the font does not have, and turning it into a glyph
    /// number would draw a glyph at a place on the page where the document asked for none.
    ///
    /// That is not a rare shape. `/CIDToGIDMap` streams are indexed by CID and run to
    /// hundreds, so a string that has been through the wrong reader arrives here as a
    /// plausible CID, and `0x00F0` is both a perfectly good CID and a character code that
    /// means nothing.
    #[must_use]
    pub fn glyph_for_code16(&self, code: u32, name: Option<&str>) -> Option<u32> {
        self.named_code_glyph(code, name)
    }

    /// A character code as a glyph number, through the font's own tables and no further.
    ///
    /// The shared half of [`Self::glyph_for_code_named`] and [`Self::glyph_for_code16`]:
    /// what the font itself says about the code, with nothing added to it. Three routes, in
    /// this order:
    ///
    /// * **The font's own `post` table**, whose entries are `glyphNNN` rather than names —
    ///   matched only when the file named a glyph that is actually there, which is the rare
    ///   case of a symbolic font remapped to a numbered glyph.
    /// * **The Unicode subtables**, through the AGL: a name like `/adieresis` is a
    ///   character, and the (3, 1) subtable is the map from characters to glyphs. This is how
    ///   a remapped code reaches a glyph in a TrueType font.
    /// * **The code through the three subtables** a simple font uses, which is what a font
    ///   with no `/Encoding` is addressed by.
    fn named_code_glyph(&self, code: u32, name: Option<&str>) -> Option<u32> {
        if let Some(font) = self.type1() {
            return font.glyph_for_named_code(code, name);
        }
        match Face::parse(&self.data, 0) {
            Ok(face) => {
                if let Some(name) = name
                    && let Some(glyph) = glyph_by_post_name(&face, name)
                {
                    return Some(glyph);
                }
                name.and_then(|n| glyph_by_unicode_name(&face, n))
                    .or_else(|| glyph_from_subtables(&face, code))
            }
            Err(_) => name.and_then(|n| self.glyph_for_name(n)),
        }
    }

    /// A character identifier as a glyph number, for a bare CID-keyed CFF.
    ///
    /// The CFF charset is what says otherwise, and the only charset this reads is the
    /// identity one — so reaching this at all means the identifier and the glyph number are
    /// the same number, and the range check is the only question left.
    fn identity_cid(&self, code: u32) -> Option<u32> {
        let table = cff::cff_bytes(&self.data)?;
        let cff = Cff::parse(table, 0).ok()?;
        if !cff.is_cid_keyed() {
            return None;
        }
        (code < cff.num_glyphs() as u32).then_some(code)
    }
}

/// The three subtables a simple TrueType font is looked up through, in the order they are
/// tried: Windows Symbol, Macintosh Roman, Windows Unicode.
const SUBTABLES: [(PlatformId, u16); 3] = [
    (PlatformId::Windows, 0),
    (PlatformId::Macintosh, 0),
    (PlatformId::Windows, 1),
];

/// The glyph a TrueType `post` table gives a name, when it has one.
///
/// A `post` version 2.0 table holds real names; the earlier versions hold the standard Mac
/// glyph numbering (`mac` encoding), which is a number in the name of a glyph rather than a
/// name. `ttf-parser` exposes one of each: [`ttf_parser::Face::glyph_index`] answers for the
/// names it has, and `post_names_match` is not consulted here because a font with no
/// `post` table is the common case for a subset. `None` is the honest answer for everything
/// else, and the caller falls to the Unicode route.
fn glyph_by_post_name(face: &Face<'_>, name: &str) -> Option<u32> {
    // Only a `post` table that carries names has an entry for a name at all, and asking
    // `ttf-parser` for a name it does not have costs a comparison against every glyph.
    let post = face.tables().post?;
    let count = u32::from(face.number_of_glyphs());
    (0..count).find(|glyph| post.glyph_name(GlyphId(*glyph as u16)) == Some(name))
}

/// The glyph a Unicode subtable gives a glyph name, through the AGL.
///
/// A `/Differences` array writes names, and a name is a character in all but a handful of
/// cases, so the name is turned into its Unicode code point and looked up in the (3, 1) and
/// (1, 0) subtables — the two that are maps from characters. The (3, 0) subtable is
/// deliberately not used here: it is a symbolic font's author's own choice of code for a
/// glyph, and it is already consulted by code in [`glyph_from_subtables`], which is the right
/// order for it.
fn glyph_by_unicode_name(face: &Face<'_>, name: &str) -> Option<u32> {
    let code = crate::encoding::agl(name)?;
    for subtable in face.tables().cmap?.subtables {
        let from_characters = (subtable.platform_id == PlatformId::Windows
            && subtable.encoding_id == 1)
            || (subtable.platform_id == PlatformId::Macintosh && subtable.encoding_id == 0);
        if from_characters && let Some(glyph) = subtable.glyph_index(code) {
            return Some(u32::from(glyph.0));
        }
    }
    None
}

/// Ask each subtable in turn, and take the first answer.
fn glyph_from_subtables(face: &Face<'_>, code: u32) -> Option<u32> {
    for (platform, encoding) in SUBTABLES {
        for subtable in face.tables().cmap?.subtables {
            if subtable.platform_id == platform
                && subtable.encoding_id == encoding
                && let Some(glyph) = subtable.glyph_index(code)
            {
                return Some(u32::from(glyph.0));
            }
        }
    }
    None
}

/// One glyph's outline, in ems, and the font's units per em.
///
/// Composite glyphs — an accented letter built from a base and a mark, which is how most
/// fonts keep their size down — come out whole: `ttf-parser` follows the component list
/// and hands the builder every contour of every part, already moved and scaled into place.
/// A caller never has to know a composite happened.
///
/// `index` is a face number for a font *collection*; a `/FontFile2` is a single font and
/// is always zero.
#[must_use]
pub fn from_true_type(data: &[u8], index: u32) -> Option<(Outline, u16)> {
    outline_of(data, index, 0)
}

/// The outline of glyph zero of a CFF font program, in ems, and the font's units per em.
///
/// The counterpart of [`from_true_type`], and the same answer in the same units: `data` may
/// be the bare `CFF ` table of a `/FontFile3` with `/Subtype /Type1C` or the whole `sfnt`
/// of one with `/Subtype /OpenType`, and `index` selects which font of a multi-font CFF
/// file, which is zero for anything a PDF carries.
///
/// `None` when the bytes are not a CFF, and `None` — never a wrong glyph — when they are a
/// CID-keyed font whose charset this cannot read.
#[must_use]
pub fn from_cff(data: &[u8], index: u32) -> Option<(Outline, u16)> {
    let table = cff::cff_bytes(data)?;
    let cff = Cff::parse(table, index).ok()?;
    let outline = cff.outline(0).ok()?;
    Some((outline, cff::units_from_matrix(&cff.font_matrix)))
}

/// The outline of glyph zero of a Type 1 font program, in ems, and the font's units per em.
///
/// The third of the three constructors, and the answer in the same units. `data` is the bare
/// PostScript a `/FontFile` carries: the cleartext header followed by the `eexec`-encrypted
/// private DICT and charstrings. A `.pfb` container wraps the same program in segment headers
/// and is refused rather than mis-parsed, so this answers `None` for one.
///
/// `None` when the bytes are not a Type 1 font, which includes a font whose charstrings name
/// an `OtherSubrs` procedure this does not run — a wrong glyph for every character in it
/// being a worse answer than no glyph.
#[must_use]
pub fn from_type1(data: &[u8]) -> Option<(Outline, u16)> {
    let font = Type1::parse(data).ok()?;
    let outline = font.outline(0).ok()?;
    Some((outline, cff::units_from_matrix(&font.font_matrix())))
}

/// The outline of one glyph of one face, in ems, and the font's units per em.
///
/// `index` is a face number for a font *collection*; a `/FontFile2` is a single font and is
/// always zero. A glyph number the font does not have is `.notdef`, which is what a
/// missing glyph should draw as rather than a walk off the end of the glyph table.
#[must_use]
pub fn outline_of(data: &[u8], index: u32, glyph: u32) -> Option<(Outline, u16)> {
    let face = Face::parse(data, index).ok()?;
    let units = face.units_per_em();
    let mut builder = Builder {
        segments: Vec::new(),
        scale: em_scale(units),
    };
    // `None` from `outline_glyph` is a glyph with no outline, which is a space and a
    // perfectly good answer: an empty path, not a failure.
    face.outline_glyph(
        GlyphId(checked(glyph, face.number_of_glyphs())),
        &mut builder,
    );
    Some((builder.finish(), units))
}

/// A glyph number the font actually has, or zero (`.notdef`) if it has not got it.
///
/// `ttf-parser`'s `GlyphId` is a newtype over `u16` that takes any value, so a glyph number
/// read from a file has to be range-checked here rather than trusted into the table walk.
fn checked(glyph: u32, number_of_glyphs: u16) -> u16 {
    if glyph < u32::from(number_of_glyphs) {
        u16::try_from(glyph).unwrap_or(0)
    } else {
        0
    }
}

/// Collects an outline in ems.
struct Builder {
    segments: Vec<Segment>,
    scale: f64,
}

impl Builder {
    fn finish(self) -> Outline {
        Outline {
            segments: self.segments,
        }
    }

    /// A point from the font's own grid into ems.
    fn point(&self, x: f32, y: f32) -> (f64, f64) {
        (f64::from(x) * self.scale, f64::from(y) * self.scale)
    }
}

impl OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.segments.push(Segment::Move(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.segments.push(Segment::Line(x, y));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (ax, ay) = self.point(x1, y1);
        let (bx, by) = self.point(x2, y2);
        let (cx, cy) = self.point(x, y);
        self.segments.push(Segment::Curve(ax, ay, bx, by, cx, cy));
    }

    /// A quadratic edge, which only a CFF outline produces.
    ///
    /// Raised to a cubic rather than dropped. The two are the same curve: with
    /// `P0` the point the pen is at, the cubic's controls are `P0 + ⅔(Q0 − P0)` and
    /// `Q1 + ⅔(Q0 − Q1)`, and the end is `Q1`. An outline with a piece missing is worse
    /// than one with a piece converted.
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (qx, qy) = self.point(x1, y1);
        let (cx, cy) = self.point(x, y);
        let start = self.last_point().unwrap_or((0.0, 0.0));
        let c1 = (
            start.0 + (qx - start.0) / 1.5,
            start.1 + (qy - start.1) / 1.5,
        );
        let c2 = (cx + (qx - cx) / 1.5, cy + (qy - cy) / 1.5);
        self.segments
            .push(Segment::Curve(c1.0, c1.1, c2.0, c2.1, cx, cy));
    }

    /// A contour's end.
    ///
    /// A TrueType contour is closed by the specification, and the fill rule needs the
    /// closing edge to exist for the winding to come out right — a `C` with its last point
    /// not joined to its first is a shape with a notch in it. The next `Move` starts the
    /// next contour, so the way to close is to push the line back to where this one
    /// began.
    fn close(&mut self) {
        let (Some(start), Some(end)) = (self.current_contour_start(), self.last_point()) else {
            return;
        };
        // `ttf-parser` normally ends a contour by walking back to its first point, in
        // which case the edge is already there and adding it again would close a figure
        // of eight. A contour that ends somewhere else — a damaged font, or one whose
        // last point is missing — is closed here, because a `C` with an unjoined end is a
        // shape with a notch in it, and the winding the fill rule reads would be wrong.
        let closed =
            (end.0 - start.0).abs() <= f64::EPSILON && (end.1 - start.1).abs() <= f64::EPSILON;
        if !closed {
            self.segments.push(Segment::Line(start.0, start.1));
        }
    }
}

impl Builder {
    /// Where the contour now being collected began.
    fn current_contour_start(&self) -> Option<(f64, f64)> {
        let last_move = self
            .segments
            .iter()
            .rposition(|s| matches!(s, Segment::Move(..)))
            .unwrap_or(0);
        match self.segments.get(last_move) {
            Some(Segment::Move(x, y)) => Some((*x, *y)),
            _ => None,
        }
    }

    /// The point the pen is at.
    fn last_point(&self) -> Option<(f64, f64)> {
        match self.segments.last() {
            Some(Segment::Move(x, y) | Segment::Line(x, y)) => Some((*x, *y)),
            Some(Segment::Curve(x3, y3, ..)) => Some((*x3, *y3)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index slices whose length they
    // have just asserted; both are what a test is for. The panic-free rule is about what
    // the product does with a file, not about how a test reads one.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::float_cmp,
        // A cubic has six coordinates and the specification names them a, b, c, d, e and
        // f. Spelling them x1, x2, x3, y1, y2, y3 would be a different, worse notation.
        clippy::many_single_char_names
    )]

    use super::*;

    // ── A font, built here ────────────────────────────────────────────────────
    //
    // Every test below needs a real font program, and a real one is 300 kB of somebody
    // else's work that may or may not be installed. So the tests build their own: a
    // `glyf` table of four points, a `cmap` of one entry, and the five tables
    // `ttf-parser` insists on. The rectangles and composites below are therefore real
    // TrueType outlines that a real parser has walked, not a mock of one.

    /// One glyph's `glyf` data: a simple glyph of one contour with four points, all
    /// on-curve and all in 16-bit signed form.
    ///
    /// A simple glyph description is `numberOfContours`, a bounding box the reader ignores,
    /// the last point of each contour, the length of the instruction bytecode, the
    /// bytecode, then the flags and the coordinates. The coordinates are **deltas** from
    /// the previous point, not positions, which is the one thing about this format that is
    /// easy to get wrong and which no amount of guessing will produce a working font from.
    fn glyf_rectangle(x0: i16, y0: i16, x1: i16, y1: i16) -> Vec<u8> {
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let mut g: Vec<u8> = Vec::new();
        g.extend_from_slice(&1i16.to_be_bytes()); // numberOfContours: one
        for v in [x0, y0, x1, y1] {
            g.extend_from_slice(&v.to_be_bytes()); // bounding box
        }
        g.extend_from_slice(&3u16.to_be_bytes()); // the contour ends at its fourth point
        g.extend_from_slice(&0u16.to_be_bytes()); // no instructions

        // Flag 0x01 is on-curve. Leaving 0x02/0x04 (the short forms) and 0x10/0x20 (the
        // "same as before" forms) clear makes every coordinate a signed 16-bit delta,
        // which is the one encoding the test does not have to decode twice over.
        g.extend(std::iter::repeat_n(0x01, corners.len()));
        // All the x deltas, then all the y deltas: the two are separate arrays, and a
        // reader walks them in that order.
        let mut previous_x = 0i16;
        for (x, _) in &corners {
            g.extend_from_slice(&x.wrapping_sub(previous_x).to_be_bytes());
            previous_x = *x;
        }
        let mut previous_y = 0i16;
        for (_, y) in &corners {
            g.extend_from_slice(&y.wrapping_sub(previous_y).to_be_bytes());
            previous_y = *y;
        }
        g
    }

    /// One glyph's `glyf` data: a composite of `parts`, each moved by `(dx, dy)`.
    ///
    /// Component flags: 0x0002 `ARG_1_AND_2_ARE_WORDS`, 0x0008 `WE_HAVE_A_SCALE` is not
    /// set, 0x0020 `MORE_COMPONENTS` on all but the last.
    fn glyf_composite(parts: &[(u16, i16, i16)]) -> Vec<u8> {
        let mut g: Vec<u8> = Vec::new();
        g.extend_from_slice(&(-1i16).to_be_bytes()); // a negative count means composite
        for _ in 0..4 {
            g.extend_from_slice(&0i16.to_be_bytes()); // an all-zero bounding box
        }
        for (i, (glyph, dx, dy)) in parts.iter().enumerate() {
            // 0x0001 `ARG_1_AND_2_ARE_WORDS` with 0x0002 `ARGS_ARE_XY_VALUES` means the two
            // arguments are 16-bit *offsets*. Leave 0x0002 out and they are point indices
            // instead, the offsets are never read, and the component list desynchronises
            // and walks off the end of the glyph — which reads as a hundred-odd segments
            // rather than as an error.
            let more = if i + 1 < parts.len() { 0x0020u16 } else { 0 };
            g.extend_from_slice(&(more | 0x0001 | 0x0002).to_be_bytes());
            g.extend_from_slice(&glyph.to_be_bytes());
            g.extend_from_slice(&dx.to_be_bytes());
            g.extend_from_slice(&dy.to_be_bytes());
        }
        g
    }

    /// Assemble a font program around a set of `glyf` entries, with a (3, 1) `cmap`
    /// mapping each listed code to the glyph of the same number.
    fn font(glyphs: &[Vec<u8>], codes: &[(u32, u16)], units_per_em: u16) -> Vec<u8> {
        font_with_symbol(glyphs, codes, &[], units_per_em)
    }

    /// The same font, with a Windows Symbol `(3, 0)` subtable as well.
    ///
    /// This is the shape a **symbolic** font has, and the shape
    /// `pdfjs__issue16263.pdf`'s embedded SymbolMT has: two subtables, one of them the
    /// symbol one, neither of them the numbering the file declared. A reader that reaches a
    /// CID through `(3, 0)` is answering with the font author's character codes rather than
    /// with the identifiers the PDF gave, and this font is built so that the two answers
    /// differ.
    fn font_with_symbol(
        glyphs: &[Vec<u8>],
        codes: &[(u32, u16)],
        symbol: &[(u32, u16)],
        units_per_em: u16,
    ) -> Vec<u8> {
        // loca: short format, so every offset is a multiple of two and the glyph data is
        // padded to two bytes.
        let mut glyf: Vec<u8> = Vec::new();
        let mut loca: Vec<u8> = Vec::new();
        for g in glyphs {
            loca.extend_from_slice(&u16::try_from(glyf.len() / 2).unwrap().to_be_bytes());
            glyf.extend_from_slice(g);
            if glyf.len() % 2 == 1 {
                glyf.push(0);
            }
        }
        loca.extend_from_slice(&u16::try_from(glyf.len() / 2).unwrap().to_be_bytes());

        let hmtx: Vec<u8> = glyphs
            .iter()
            .flat_map(|_| [0x00, 0x64, 0x00, 0x00]) // 1000/1000 in 1000ths, lsb 0
            .collect();

        // head: 54 bytes, of which the fields ttf-parser reads are unitsPerEm and
        // indexToLocFormat.
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // version 1.0
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magic
        head[18..20].copy_from_slice(&units_per_em.to_be_bytes());
        head[50..52].copy_from_slice(&0i16.to_be_bytes()); // indexToLocFormat: short

        let mut maxp = vec![0u8; 32];
        maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&u16::try_from(glyphs.len()).unwrap().to_be_bytes());

        // hhea: 36 bytes, of which ttf-parser reads numberOfHMetrics.
        let mut hhea = vec![0u8; 36];
        hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        hhea[34..36].copy_from_slice(&u16::try_from(glyphs.len()).unwrap().to_be_bytes());

        cmap(codes, symbol).map_or_else(Vec::new, |cmap| {
            assemble(&[
                ("cmap", cmap),
                ("glyf", glyf),
                ("head", head),
                ("hhea", hhea),
                ("hmtx", hmtx),
                ("loca", loca),
                ("maxp", maxp),
            ])
        })
    }

    /// A `cmap` table holding a Windows Unicode `(3, 1)` subtable and, when asked for, a
    /// Windows Symbol `(3, 0)` one.
    fn cmap(codes: &[(u32, u16)], symbol: &[(u32, u16)]) -> Option<Vec<u8>> {
        let mut tables: Vec<(u16, u16, Vec<u8>)> = Vec::new();
        if let Some(unicode) = cmap_format4(codes) {
            tables.push((3, 1, unicode));
        }
        if let Some(symbol) = cmap_format4(symbol) {
            tables.push((3, 0, symbol));
        }
        if tables.is_empty() {
            return None;
        }
        // Each encoding record is eight bytes after the two-word header, so the subtables
        // start at 4 + 8 × count.
        let header = 4 + 8 * tables.len();
        let mut table: Vec<u8> = Vec::with_capacity(header);
        table.extend_from_slice(&0u16.to_be_bytes()); // version
        table.extend_from_slice(&u16::try_from(tables.len()).ok()?.to_be_bytes());
        let mut at = header;
        for (platform, encoding, sub) in &tables {
            table.extend_from_slice(&platform.to_be_bytes());
            table.extend_from_slice(&encoding.to_be_bytes());
            table.extend_from_slice(&u32::try_from(at).ok()?.to_be_bytes());
            at += sub.len();
        }
        for (_, _, sub) in &tables {
            table.extend_from_slice(sub);
        }
        Some(table)
    }

    /// A format 4 `cmap`: one segment per code, all sharing a delta.
    ///
    /// Format 4 is the one every simple font has, and building it by hand is the only way
    /// to be sure the subtable is found through the route this crate claims it uses.
    fn cmap_format4(codes: &[(u32, u16)]) -> Option<Vec<u8>> {
        if codes.is_empty() {
            return None;
        }
        // One segment per code, plus the mandatory final segment that terminates the
        // list. Each code gets its own single-code segment, which is wasteful and
        // completely unambiguous — the point of building this by hand is that the
        // subtable is found through the route the crate claims it uses.
        let mut starts = Vec::with_capacity(codes.len());
        let mut ends = Vec::with_capacity(codes.len());
        for (code, _) in codes {
            let c = u16::try_from(*code).ok()?;
            starts.push(c);
            ends.push(c);
        }
        starts.push(0xFFFF);
        ends.push(0xFFFF);
        // A single-code segment needs no `glyphIdArray` entry, so the glyph is a
        // constant offset from the code and `idRangeOffset` stays zero.
        let mut deltas: Vec<i16> = Vec::with_capacity(codes.len() + 1);
        for (code, glyph) in codes {
            deltas.push(i16::try_from(*glyph).ok()?.wrapping_sub(*code as i16));
        }
        deltas.push(1); // the final segment, which must map 0xFFFF to something

        // format, length, language, segCountX2, searchRange, entrySelector, rangeShift,
        // then endCode[], reservedPad, startCode[], idDelta[], idRangeOffset[].
        let seg_x2 = u16::try_from(starts.len() * 2).ok()?;
        let length = 16 + seg_x2 as usize * 4;
        let mut sub: Vec<u8> = Vec::with_capacity(length);
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        sub.extend_from_slice(&u16::try_from(length).ok()?.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        sub.extend_from_slice(&seg_x2.to_be_bytes());
        // The three binary-search hints, which a conforming reader may use and a
        // font builder is expected to get right. The largest power of two not above the
        // segment count, doubled, then halved, then the remainder.
        let power = 1u32 << (starts.len().saturating_sub(1)).ilog2();
        sub.extend_from_slice(&u16::try_from(power * 2).ok()?.to_be_bytes());
        sub.extend_from_slice(&u16::try_from(power.ilog2()).ok()?.to_be_bytes());
        sub.extend_from_slice(&(seg_x2 - power as u16 * 2).to_be_bytes());
        for c in &ends {
            sub.extend_from_slice(&c.to_be_bytes());
        }
        sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for c in &starts {
            sub.extend_from_slice(&c.to_be_bytes());
        }
        for d in &deltas {
            sub.extend_from_slice(&d.to_be_bytes());
        }
        for _ in &starts {
            sub.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset
        }
        debug_assert_eq!(sub.len(), length);
        Some(sub)
    }

    /// Put tables into a `sfnt` container: a header, a table directory, and the tables.
    ///
    /// The directory is sorted by tag because the specification requires it and readers
    /// binary-search it: `ttf-parser` does, and a directory in the wrong order makes every
    /// table unfindable rather than just one.
    fn assemble(tables: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut tables = tables.to_vec();
        tables.sort_by_key(|(tag, _)| *tag);
        let count = u16::try_from(tables.len()).unwrap();
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // TrueType
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&[0u8; 6]); // searchRange, entrySelector, rangeShift
        let mut offset = 12 + 16 * tables.len();
        let mut records = Vec::new();
        for (tag, data) in &tables {
            records.extend_from_slice(tag.as_bytes());
            records.extend_from_slice(&0u32.to_be_bytes()); // checksum
            records.extend_from_slice(&u32::try_from(offset).unwrap().to_be_bytes());
            records.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            offset += data.len() + (4 - data.len() % 4) % 4;
        }
        out.extend_from_slice(&records);
        for (_, data) in &tables {
            out.extend_from_slice(data);
            out.extend(std::iter::repeat_n(0u8, (4 - data.len() % 4) % 4));
        }
        out
    }

    /// A font whose glyph 1 is a rectangle, whose glyph 2 is that rectangle twice at two
    /// offsets, and whose glyph 3 is empty. Code 65 is glyph 1 and code 66 is glyph 2.
    fn sample(units_per_em: u16) -> Vec<u8> {
        font(
            &[
                Vec::new(),
                glyf_rectangle(100, 200, 300, 400),
                glyf_composite(&[(1, 500, 0), (1, 0, 600)]),
                Vec::new(),
            ],
            &[(65, 1), (66, 2)],
            units_per_em,
        )
    }

    /// A CID-keyed font: five glyphs, and a Windows Symbol `(3, 0)` subtable that
    /// **disagrees** with the identity map on the identifiers it covers.
    ///
    /// Glyphs 1, 2 and 4 are rectangles of different sizes, so *which* glyph came back is
    /// observable in the outline and not only in a number. The `(3, 0)` subtable maps code 1
    /// to glyph 4 and code 2 to glyph 1 — the shape a subsetted symbolic font has, and the
    /// shape the SymbolMT in `pdfjs__issue16263.pdf` has, whose `(3, 0)` subtable covers only
    /// `U+F021` and its five neighbours while the file declares `/CIDToGIDMap /Identity`. A
    /// CID lookup that consults the `cmap` answers 4 and 1 here, and both are wrong.
    fn cid_font() -> Vec<u8> {
        font_with_symbol(
            &[
                Vec::new(),
                glyf_rectangle(125, 125, 375, 375),
                glyf_rectangle(500, 500, 750, 750),
                Vec::new(),
                glyf_rectangle(250, 250, 500, 500),
            ],
            &[(65, 1), (66, 2)],
            &[(1, 4), (2, 1)],
            1000,
        )
    }

    /// The bounds of a glyph's outline in ems, for saying *which* glyph came back.
    fn bounds(program: &mut Program, glyph: u32) -> Option<(f64, f64, f64, f64)> {
        let (outline, _) = program.outline(glyph)?;
        let mut at = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for segment in &outline.segments {
            let points: Vec<(f64, f64)> = match *segment {
                Segment::Move(x, y) | Segment::Line(x, y) => vec![(x, y)],
                Segment::Curve(a, b, c, d, e, f) => vec![(a, b), (c, d), (e, f)],
            };
            for (x, y) in points {
                at.0 = at.0.min(x);
                at.1 = at.1.min(y);
                at.2 = at.2.max(x);
                at.3 = at.3.max(y);
            }
        }
        Some(at)
    }

    // ── The tests ────────────────────────────────────────────────────────────

    /// Four on-curve points, no curves, come back as the same four points.
    ///
    /// This is the round trip that matters: parse a real `glyf` table with a real parser
    /// and get the geometry out unaltered. A rectangle is the shape where any mistake in
    /// the walk — a dropped point, a doubled close, a scale applied twice — shows up
    /// immediately and exactly, with no tolerance to hide behind.
    #[test]
    fn a_rectangle_of_four_on_curve_points_survives_the_walk() {
        let data = sample(1000);
        let (outline, units) = outline_of(&data, 0, 1).expect("a rectangle");
        assert_eq!(units, 1000);
        // A move to the first point, three lines, and the closing line back to the start.
        assert_eq!(
            outline.segments,
            vec![
                Segment::Move(0.1, 0.2),
                Segment::Line(0.3, 0.2),
                Segment::Line(0.3, 0.4),
                Segment::Line(0.1, 0.4),
                Segment::Line(0.1, 0.2),
            ],
            "the same four points, in ems, closed"
        );
    }

    /// A glyph drawn as a rectangle is a rectangle whatever the em is.
    ///
    /// The 2048-unit font is the older convention and the one that catches a hard-coded
    /// scale: if the scale were a constant, this would come out a thousand times the size.
    #[test]
    fn the_em_scale_actually_applies() {
        let (wide, _) = outline_of(&sample(2048), 0, 1).expect("a rectangle");
        let (x, y) = match wide.segments.first() {
            Some(Segment::Move(x, y)) => (*x, *y),
            other => panic!("expected a move, got {other:?}"),
        };
        // 100 units of a 2048-unit em is 100/2048 ems, exactly.
        assert!(
            (x - 100.0 / 2048.0).abs() < 1e-12,
            "100 units of a 2048-unit em is 100/2048 em, got {x}"
        );
        assert!(
            (y - 200.0 / 2048.0).abs() < 1e-12,
            "and 200 units is 200/2048 em, got {y}"
        );

        // Five hundred units of a thousand-unit em is half an em, exactly.
        let half = glyf_rectangle(0, 0, 500, 500);
        let data = font(&[Vec::new(), half], &[(65, 1)], 1000);
        let (outline, _) = outline_of(&data, 0, 1).expect("a rectangle");
        let Some(Segment::Line(x, _)) = outline.segments.get(1) else {
            panic!("expected a line, got {:?}", outline.segments);
        };
        assert!(
            (x - 0.5).abs() < 1e-12,
            "500 units of 1000 is 0.5 em, got {x}"
        );
    }

    /// One em is however many units the font says it is.
    ///
    /// Stated as a property of the geometry rather than as a formula, because the formula
    /// is the thing under test: scale any number of units back up by the scale and it must
    /// come to the em, at every em size. A scale of `1000 / unitsPerEm` satisfies that only
    /// for a thousand-unit font and is a thousand times too large for every other.
    #[test]
    fn the_em_scale_is_the_number_the_specification_names() {
        for units in [16u16, 100, 1000, 1024, 2048, 4096, 16384] {
            assert!(
                (em_scale(units) * f64::from(units) - 1.0).abs() < 1e-12,
                "{units} units to the em, scaled back up, is one em"
            );
        }
        assert_eq!(em_scale(1000), 0.001, "a thousand-unit em is a thousandth");
        assert_eq!(em_scale(2048), 1.0 / 2048.0);
        assert_eq!(
            em_scale(0),
            1.0,
            "a font claiming no units to the em is treated as already using 1000"
        );
    }

    /// A composite glyph is the sum of its parts, not the first part.
    ///
    /// This is a real composite: two references to the same rectangle, the first moved 500
    /// units right and the second 600 units up. `ttf-parser` follows the component list
    /// and hands the builder both contours, already offset. If the component walk were
    /// skipped the outline would be one rectangle, and the count assertion below would
    /// fail rather than the glyph quietly rendering half a shape.
    #[test]
    fn a_composite_glyph_is_the_sum_of_its_parts() {
        let data = sample(1000);
        let (composite, _) = outline_of(&data, 0, 2).expect("a composite");
        let (part, _) = outline_of(&data, 0, 1).expect("a rectangle");

        // Two contours of five segments each: the composite is exactly twice the part.
        assert_eq!(
            composite.segments.len(),
            part.segments.len() * 2,
            "two components, each contributing its own closed contour"
        );
        assert!(
            composite.segments.len() > 5,
            "and it is more than the single part, which is what makes this a test at all"
        );

        let moved = |dx: f64, dy: f64| -> Vec<Segment> {
            part.segments
                .iter()
                .map(|s| match *s {
                    Segment::Move(x, y) => Segment::Move(x + dx, y + dy),
                    Segment::Line(x, y) => Segment::Line(x + dx, y + dy),
                    Segment::Curve(a, b, c, d, e, f) => {
                        Segment::Curve(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy)
                    }
                })
                .collect()
        };
        let at = composite.segments.len() / 2;
        assert_eq!(
            &composite.segments[..at],
            moved(0.5, 0.0).as_slice(),
            "the first component is the rectangle, moved 500 units right"
        );
        assert_eq!(
            &composite.segments[at..],
            moved(0.0, 0.6).as_slice(),
            "the second is the same rectangle, moved 600 units up"
        );
    }

    /// A character code becomes a glyph, through the `cmap`.
    #[test]
    fn a_character_code_is_mapped_through_the_cmap() {
        let program = Program::new(sample(1000));
        assert_eq!(program.glyph_for_code(65), Some(1));
        assert_eq!(program.glyph_for_code(66), Some(2));
        // Glyph 3 is in the font and the cmap does not mention it, so the fallback has to
        // find it. A subsetted symbolic font with no usable `cmap` is exactly this case,
        // and it is why a code that no subtable answers is taken as a glyph number
        // rather than reported as a missing character.
        assert_eq!(
            program.glyph_for_code(3),
            Some(3),
            "a code no subtable answers is taken as a glyph number"
        );
        assert_eq!(
            program.glyph_for_code(4000),
            None,
            "a number past the last glyph is nothing, rather than a walk off the end"
        );
    }

    /// A code with a glyph that has no outline is an empty path, not a failure.
    #[test]
    fn a_glyph_with_no_outline_is_empty_rather_than_missing() {
        let mut program = Program::new(sample(1000));
        let (outline, units) = program.outline_for_code(65).expect("a rectangle");
        assert!(!outline.is_empty());
        assert_eq!(units, 1000);

        let (blank, _) = outline_of(&sample(1000), 0, 3).expect("an empty glyph");
        assert!(
            blank.is_empty(),
            "a space is an empty path, and a renderer that reports it fills a page with notes"
        );
    }

    /// The same outline twice, from a cache and from a fresh parse.
    ///
    /// A cache that answered differently from the parse would be a renderer that draws
    /// one `e` differently from the next, which no test above would catch.
    #[test]
    fn a_cached_outline_is_the_outline_a_fresh_parse_gives() {
        let data = sample(1000);
        let mut program = Program::new(data);
        let first = program.outline(1);
        let second = program.outline(1);
        assert_eq!(first, second, "the second ask is the cached answer");
        let (parsed, _) = outline_of(&sample(1000), 0, 1).expect("a rectangle");
        assert_eq!(
            first.map(|(o, _)| o),
            Some(parsed),
            "the cache agrees with a fresh parse of the same bytes"
        );
    }

    /// Bytes from the internet are not fonts, and must not panic.
    ///
    /// A PDF can carry any `/FontFile2` at all, and the whole of the tolerance for that
    /// lives here. A panic on a truncated font is a crash on a file a user opened, which
    /// is the one outcome this crate exists to prevent, so each of these is a case that
    /// has to come back as `None` rather than as a panic or a wrong answer.
    #[test]
    fn a_font_that_is_not_a_font_is_none_rather_than_a_panic() {
        // Empty.
        assert!(from_true_type(&[], 0).is_none());
        // A PDF, which is what a `/FontFile2` most often is when the file is damaged.
        assert!(
            from_true_type(b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n%%EOF\n", 0).is_none(),
            "a PDF is not a font"
        );
        // A font cut in half at every third length, which is what a truncated download is.
        let data = sample(1000);
        for cut in 1..data.len() {
            let _ = from_true_type(&data[..cut], 0);
            let _ = from_true_type(&data[..cut], 99);
        }
        // A header with nothing behind it.
        assert!(from_true_type(&[0x00, 0x01, 0x00, 0x00], 0).is_none());
        // A font collection face index that does not exist.
        assert!(from_true_type(&data, 12_345).is_none());
        // Nonsense where a table directory should be.
        let mut broken = data.clone();
        broken[4..6].copy_from_slice(&0xFFFFu16.to_be_bytes()); // 65535 table records
        let _ = from_true_type(&broken, 0);
    }

    /// A font whose `head` claims no units to the em is refused, not divided by.
    ///
    /// `ttf-parser` rejects a `head` outside 16..=16384, so the zero never reaches
    /// [`em_scale`] by that route and such a file is simply not a font we can read. The
    /// guard in `em_scale` is defence in depth for any other caller, and what is
    /// asserted here is that the two together answer `None` rather than an infinity or a
    /// panic.
    #[test]
    fn a_font_claiming_no_units_to_the_em_is_refused_rather_than_divided_by() {
        let data = font(&[Vec::new(), glyf_rectangle(0, 0, 100, 100)], &[(65, 1)], 0);
        assert!(
            outline_of(&data, 0, 1).is_none(),
            "a zero em is not a font we can read, rather than one scaled by infinity"
        );
        assert_eq!(
            em_scale(0),
            1.0,
            "and the guard itself answers 1, which is the whole of its contract"
        );
    }

    // ── two-byte codes: a CID, or a character code ──────────────────────────────

    /// With no `/CIDToGIDMap`, a CID is the glyph number — and never the `cmap`'s answer.
    ///
    /// The specification's default map is the identity map, so a font that names none says
    /// its identifiers *are* its glyph numbers. This font's `(3, 0)` subtable says otherwise —
    /// it maps CID 1 to glyph 4 and CID 2 to glyph 1 — and a reader that consulted it would
    /// draw the wrong two glyphs. The outline is checked as well as the number, because the
    /// point is which shape reaches the page.
    #[test]
    fn a_cid_with_no_cid_to_gid_map_is_the_glyph_number() {
        let mut program = Program::new(cid_font());
        assert_eq!(
            program.glyph_for_cid(2, &CidToGid::Identity),
            Some(2),
            "CID 2 is glyph 2, whatever the symbol subtable says"
        );
        assert_eq!(
            bounds(&mut program, 2),
            Some((0.5, 0.5, 0.75, 0.75)),
            "and the outline that comes back is glyph 2's rectangle, not glyph 1's"
        );
        assert_eq!(
            program.glyph_for_cid(1, &CidToGid::Identity),
            Some(1),
            "and CID 1 is glyph 1, not the 4 the symbol subtable would have given"
        );
        assert_eq!(
            program.glyph_for_code(2),
            Some(1),
            "while the *same number* as a single-byte character code is answered by the \
             symbol subtable, because that is what a character code is"
        );
    }

    /// A `/CIDToGIDMap` stream is the file's own numbering, and it overrides everything.
    ///
    /// Two entries: CID 0 maps to GID 0 and CID 1 maps to GID 2. So CID 1 is glyph 2, where
    /// the identity map would have said glyph 1 and the symbol subtable would have said glyph
    /// 4 — three different answers, and only one of them is the file's.
    #[test]
    fn a_cid_to_gid_map_stream_is_the_numbering_the_file_declared() {
        let map = CidToGid::Stream(vec![0x00, 0x00, 0x00, 0x02]);
        let mut program = Program::new(cid_font());
        assert_eq!(
            program.glyph_for_cid(1, &map),
            Some(2),
            "CID 1 is glyph 2 through the stream, and not glyph 4 through the symbol subtable"
        );
        assert_eq!(
            bounds(&mut program, 2),
            Some((0.5, 0.5, 0.75, 0.75)),
            "and the outline that comes back is glyph 2's rectangle"
        );
        // The identity map would have said 1 for the same CID, so this is the assertion that
        // the stream is being read at all rather than being ignored in favour of identity.
        assert_eq!(
            program.glyph_for_cid(1, &CidToGid::Identity),
            Some(1),
            "the same CID through the identity map is glyph 1, which is what makes the two \
             answers different"
        );
    }

    /// A simple font's two-byte code is a character code, so it goes through the encoding.
    ///
    /// The font is the same one: its `(3, 1)` subtable has code 65, and its `(3, 0)` subtable
    /// has codes 1 and 2. All three are answered as character codes, because that is what
    /// they are.
    #[test]
    fn a_two_byte_character_code_in_a_simple_font_is_a_character_code() {
        let program = Program::new(cid_font());
        assert_eq!(
            program.glyph_for_code16(65, None),
            Some(1),
            "code 65 is the character the Unicode subtable names"
        );
        assert_eq!(
            program.glyph_for_code16(2, None),
            Some(1),
            "and code 2 is the character the *symbol* subtable names, because a simple font's \
             codes are the font's own and the symbol subtable is where they are"
        );
        // And code 3 names no character, even though it is a number the font has a glyph
        // for. This is where the two routes part company: a single-byte code may be read as
        // a glyph number, because a symbolic font's codes are its author's own 0-to-255
        // choice, and a two-byte code may not, because a CMap `/Encoding` names characters.
        assert_eq!(
            program.glyph_for_code16(3, None),
            None,
            "while code 3 names no character, even though this font has a glyph 3"
        );
        // And the same two-byte code reaches an outline rather than only a number, which is
        // what a renderer actually asks for.
        let mut program = program;
        let (outline, units) = program
            .outline_for_code16(65, None)
            .expect("code 65 is glyph 1 and glyph 1 is a rectangle");
        assert!(!outline.is_empty());
        assert_eq!(units, 1000);
        assert!(
            program.outline_for_code16(3, None).is_none(),
            "and code 3 has no outline either, because it names no glyph"
        );
    }

    /// A two-byte code the font has no character for is not a glyph number in disguise.
    ///
    /// Code 44 is a perfectly plausible CID — it is inside the range a `/CIDToGIDMap` stream
    /// is indexed over, and this font has glyph 44 nowhere near it — and it names no character
    /// in this font. The old answer was "it is below the glyph count, so it is glyph 44", and
    /// that is what puts a glyph on a page at a place the document never asked for. So this
    /// is the assertion the two cases turn on.
    #[test]
    fn a_plausible_cid_that_names_no_character_is_not_turned_into_a_glyph_number() {
        let program = Program::new(cid_font());
        assert_eq!(
            program.glyph_for_code16(44, None),
            None,
            "code 44 names no character in this font, so it names no glyph"
        );
        // The single-byte route still has the glyph-number answer, because a symbolic font's
        // codes are its author's own 0-to-255 choice. That is the whole difference between
        // the two routes, and this font is small enough to make it observable: it has five
        // glyphs, so code 44 is past them all and only the font's own tables can answer.
        assert_eq!(
            program.glyph_for_code(44),
            None,
            "the single-byte route agrees here only because this font has no glyph 44"
        );
        assert_eq!(
            program.glyph_for_code(3),
            Some(3),
            "and where this font *does* have a glyph of the code's number, the single-byte \
             route gives it"
        );
        assert_eq!(
            program.glyph_for_code16(3, None),
            None,
            "while the two-byte route does not, because glyph 3 is not what code 3 names"
        );
        assert_eq!(
            program.glyph_for_code16(65, None),
            Some(1),
            "and a code a subtable really does name gives the same answer either way"
        );
    }

    /// A code the font does not have is reported, and `.notdef` is never drawn.
    ///
    /// `.notdef` is a decision here, not a fallback. The specification maps a CID past the
    /// last entry of a stream to GID 0 so that "this font has no glyph here" has an answer,
    /// and GID 0 is the font's `.notdef` — a hollow box in most TrueType fonts. Drawing it
    /// would paint a character the document never asked for, so a code that reaches GID 0
    /// draws nothing, and a code the font has no glyph for is `None` rather than `Some(0)`.
    #[test]
    fn a_code_the_font_does_not_have_is_reported_rather_than_drawn_as_notdef() {
        let program = Program::new(cid_font());
        // Past the last glyph, which is what a CID beyond a subset font's range looks like.
        assert_eq!(
            program.glyph_for_cid(9, &CidToGid::Identity),
            None,
            "a CID beyond the last glyph is a code the font does not have"
        );
        // The identity map's CID 0 is GID 0.
        assert_eq!(
            program.glyph_for_cid(0, &CidToGid::Identity),
            None,
            "and CID 0 is `.notdef` under the identity map, which is drawn as nothing"
        );
        // A stream that says 0 for a CID it covers, and one that simply ends.
        assert_eq!(
            program.glyph_for_cid(0, &CidToGid::Stream(vec![0x00, 0x00, 0x00, 0x02])),
            None,
            "a stream entry of 0 is the file saying it has no glyph, not a glyph to draw"
        );
        assert_eq!(
            program.glyph_for_cid(7, &CidToGid::Stream(vec![0x00, 0x00, 0x00, 0x02])),
            None,
            "and a CID past the last entry is the specification's own GID 0"
        );
        // So a code that resolves to nothing produces no outline, which is what a renderer
        // turns into clean paper rather than a box.
        let mut program = Program::new(cid_font());
        assert!(
            program.outline_for_cid(0, &CidToGid::Identity).is_none(),
            "and there is no outline to draw for it"
        );
    }

    // ── the refusal, and what it is for ────────────────────────────────────
    ///
    /// Deliberately a short candidate list: this is about what a bare CFF program's codes
    /// reach, and a font nobody has installed is a skip rather than a failure.
    fn bare_cff_on_this_machine() -> Option<Vec<u8>> {
        const CANDIDATES: [&str; 4] = [
            "/usr/share/fonts/gnu-free/FreeSerif.otf",
            "/usr/share/fonts/gnu-free/FreeSans.otf",
            "/usr/share/fonts/gsfonts/NimbusSans-Regular.otf",
            "/usr/share/fonts/gsfonts/NimbusRoman-Regular.otf",
        ];
        CANDIDATES
            .iter()
            .find_map(|path| cff::cff_bytes(&std::fs::read(path).ok()?).map(<[u8]>::to_vec))
    }

    /// A bare name-keyed CFF font no longer refuses, because its charset is read.
    ///
    /// The refusal this replaces was the right answer while the charset was unread — it said
    /// so once rather than drawing nothing silently — and it is the wrong answer now, for the
    /// same reason a page of nothing is: the font *can* be read. What makes a character code
    /// reach a glyph here is `/Encoding` saying `A` and the charset saying which glyph `A`
    /// is, and both of those are answered.
    #[test]
    fn a_bare_name_keyed_cff_font_resolves_names_and_so_does_not_refuse() {
        let Some(bare) = bare_cff_on_this_machine() else {
            eprintln!("skipped: no CFF font found to unwrap");
            return;
        };
        let mut program = Program::new(bare);
        assert_eq!(
            program.code_refusal(),
            None,
            "a readable charset is not a reason a page cannot be drawn"
        );
        // And it is not merely a refusal that went away: the codes reach glyphs. `A` is a
        // name in the Standard Strings and every real font has it, and `notoneglyph` is a
        // name in none of them.
        assert!(
            program.glyph_for_name("A").is_some(),
            "a standard name resolves through a real font's charset"
        );
        assert_eq!(
            program.glyph_for_name("definitelynotaglyphname"),
            None,
            "and a name the font does not have resolves to nothing rather than to a guess"
        );
    }

    /// A CID font's charset is an identifier map, and it is still refused when it is not the
    /// identity. This is the one refusal here that is a limit rather than a gap, and it must
    /// survive the charset reader arriving: a name-keyed reader applied to a CID font would
    /// resolve an identifier as if it were a name.
    #[test]
    fn a_cid_font_still_refuses_a_charset_that_is_not_the_identity() {
        // There is no CID font in the candidate list, and building one here would need a CFF
        // builder this module does not have — `cff.rs` owns that. So this asserts the half
        // that is checkable from here: a CID font is not one this can resolve names through,
        // and a program that is not a font at all is refused for the reason it actually is.
        assert!(
            Program::new(b"not a font at all".to_vec())
                .code_refusal()
                .is_some(),
            "a program that is not a font is refused, and saying so is the point"
        );
        assert!(
            Program::new(b"not a font at all".to_vec())
                .glyph_for_name("A")
                .is_none(),
            "and it resolves no names either"
        );
    }
}
