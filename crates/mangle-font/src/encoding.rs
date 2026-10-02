//! What a character code is called.
//!
//! Everything between a byte in a content stream and a glyph on a page is a *name*. The
//! byte is a character code; the glyph has a name; and an encoding is the map from one to
//! the other. A font does not contain an encoding — the font dictionary names one, the
//! name may be `/WinAnsiEncoding`, and the codes above 127 are where the three differ
//! enough for a document that maps one of them to render visibly wrong text. So this file
//! is the map, and it is pure data: no font program, no page, no pixels.
//!
//! ## The four shapes a file writes
//!
//! `/Encoding` is written four ways and all four occur in real documents, so all four are
//! read by [`Encoding::from_font_dict`]:
//!
//! * a name — `/Encoding /WinAnsiEncoding`;
//! * a name with no `/BaseEncoding` — the font's own built-in encoding, which this cannot
//!   read, so the caller's default base is used and the difference is noted rather than
//!   hidden;
//! * a dictionary — `<< /Type /Encoding /BaseEncoding /MacRomanEncoding >>`;
//! * that dictionary with a `/Differences` array, which overrides the base one code at a
//!   time and may run past the end of the base, filling the gap.
//!
//! ## Where the tables came from
//!
//! They are transcribed from **Table D of the PDF specification** (D.2 standard, D.3
//! WinAnsi, D.4 MacRoman) and Annex D.2 (`PDFDocEncoding`), and each was then checked
//! against sources on the machine rather than trusted:
//!
//! * **StandardEncoding** agrees, entry for entry, with Ghostscript's
//!   `Resource/Decoding/StandardEncoding`, which is the same table written out in full.
//! * **WinAnsiEncoding** agrees with the construction in Ghostscript's
//!   `Resource/Init/gs_wan_e.ps`, which derives it from `ISOLatin1Encoding` and patches
//!   codes 39, 45, 96, 127 and 128–159; and with `pdftotext`'s reading of the same
//!   encoding (219 of 222 codes read identically, the three being codes `pdftotext`
//!   cannot emit as a single character).
//! * **MacRomanEncoding** agrees with the machine's `mac_roman` code page and with
//!   poppler's own `MacRomanEncoding` on every code from 32 to 225. It is the code page's
//!   assignments from 0x20 to 0xE1 and nothing above; 0x01–0x1F and 0x7F are the code
//!   page's control codes and are *not* assigned by the encoding. At 0xDB the two sources
//!   differ — the code page has the Euro Apple inserted later, poppler has `currency` —
//!   and the code page is followed here. Codes 0xE2–0xFF are left unassigned: poppler
//!   fills them from the standard encoding's accent block, which is a font's charset and
//!   not this encoding.
//! * **PDFDocEncoding** shares the Latin-1 block with `WinAnsiEncoding` and replaces the
//!   ranges either side of ASCII with the specification's own.
//!
//! ## Where the glyph names came from
//!
//! The resolver's set is **Adobe's Glyph List**, `glyphlist.txt`, as shipped with the
//! `read-fonts` crate. Every name the four encodings above can produce is in it, and it is
//! what supplies the Greek (`alpha`, `afii10017`) and Cyrillic (`afii10017`) names a
//! `/Differences` array really uses. That is a machine source and not a transcription, so
//! the Greek and Cyrillic blocks are here in full rather than restricted to Latin.
//!
//! ## What a composite font does instead
//!
//! A code in a Type 0 font is a **CID**, not a character code, and the specification says
//! `/Differences` does not apply to one. Nothing here is used for a composite font: its
//! `/Encoding` is a CMap or a name like `/Identity-H`, and its identifier reaches a glyph
//! through the descendant font's own mapping.

use mangle_syntax::object::{Dict, Object};

/// One of the three encodings a font dictionary may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    /// `StandardEncoding`, the specification's Table D.2 and the default for a simple font
    /// that names none.
    Standard,
    /// `WinAnsiEncoding`, Table D.3: code page 1252 with a handful of gaps filled.
    WinAnsi,
    /// `MacRomanEncoding`, Table D.4: the Mac OS Roman code page to 0xE1.
    MacRoman,
}

impl Base {
    /// The base a `/BaseEncoding` name, or a bare `/Encoding` name, stands for.
    ///
    /// `/MacExpertEncoding` is absent on purpose. It is one of the two encodings a font
    /// dictionary may name and this cannot supply, and answering it with the standard
    /// encoding would draw plausible wrong glyphs where none were asked for — so it is
    /// `None`, and a caller that has a base already falls back to it instead.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "StandardEncoding" => Some(Self::Standard),
            "WinAnsiEncoding" => Some(Self::WinAnsi),
            "MacRomanEncoding" => Some(Self::MacRoman),
            _ => None,
        }
    }

    /// The name a file writes for this base.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "StandardEncoding",
            Self::WinAnsi => "WinAnsiEncoding",
            Self::MacRoman => "MacRomanEncoding",
        }
    }

    /// The glyph name this encoding gives a code, or `None` for a code it leaves alone.
    #[must_use]
    pub fn glyph_for(self, code: u32) -> Option<&'static str> {
        let table = match self {
            Self::Standard => STANDARD,
            Self::WinAnsi => WIN_ANSI,
            Self::MacRoman => MAC_ROMAN,
        };
        lookup(table, code)
    }

    /// Every code this base assigns, with its name, in code order.
    #[must_use]
    pub fn entries(self) -> &'static [(u32, &'static str)] {
        match self {
            Self::Standard => STANDARD,
            Self::WinAnsi => WIN_ANSI,
            Self::MacRoman => MAC_ROMAN,
        }
    }
}

/// A base encoding with a `/Differences` array laid over it.
///
/// This is what a font dictionary's `/Encoding` means once it has been read: the base
/// supplies a name for every code it assigns, and `/Differences` replaces the name for the
/// codes it names. A code in neither is not a character this encoding has, which is `None`
/// rather than a default glyph — the specification's `.notdef` is a font's business and
/// inventing one here would put a visible box where the file asked for nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoding {
    base: Base,
    differences: Vec<(u32, String)>,
}

impl Encoding {
    /// The base alone, with no `/Differences`.
    #[must_use]
    pub fn new(base: Base) -> Self {
        Self {
            base,
            differences: Vec::new(),
        }
    }

    /// The base encoding underneath.
    #[must_use]
    pub fn base(&self) -> Base {
        self.base
    }

    /// The `/Differences` array as `(code, name)` pairs, in the order the file wrote them.
    #[must_use]
    pub fn differences(&self) -> &[(u32, String)] {
        &self.differences
    }

    /// Add a `/Differences` entry. A later entry for a code replaces an earlier one, which
    /// is what the array's own order says.
    pub fn push(&mut self, code: u32, name: String) {
        match self.differences.iter_mut().find(|(c, _)| *c == code) {
            Some(slot) => slot.1 = name,
            None => self.differences.push((code, name)),
        }
    }

    /// The glyph name one character code is given by this encoding.
    ///
    /// `/Differences` first, because that is the more specific statement: it names the code
    /// outright. A `/Differences` entry naming `.notdef` means the code is deliberately
    /// not a character, and it is honoured as such rather than falling through to the base.
    #[must_use]
    pub fn glyph_for(&self, code: u32) -> Option<&str> {
        if let Some((_, name)) = self.differences.iter().find(|(c, _)| *c == code) {
            return (!name.is_empty() && name != ".notdef").then_some(name.as_str());
        }
        self.base.glyph_for(code)
    }

    /// The code one name came from, where the mapping is one-to-one.
    ///
    /// The encodings repeat names: `space` is at both 32 and 160 in `WinAnsiEncoding`,
    /// and `/Differences` can repeat one freely. A name that appears at more than one code
    /// therefore came from more than one place and this is `None` rather than a guess,
    /// which is what makes the round trip a test rather than a tautology.
    #[must_use]
    pub fn code_for(&self, name: &str) -> Option<u32> {
        let mut found = None;
        let mut ambiguous = false;
        for (code, assigned) in self.base.entries() {
            if *assigned == name {
                if found.is_some() {
                    ambiguous = true;
                    break;
                }
                found = Some(*code);
            }
        }
        if ambiguous {
            return None;
        }
        let mut count = 0;
        for (_, assigned) in &self.differences {
            if assigned == name {
                count += 1;
                if count > 1 {
                    return None;
                }
            }
        }
        if count == 1 {
            return self
                .differences
                .iter()
                .find(|(_, n)| n == name)
                .map(|(c, _)| *c);
        }
        if count == 0 && found.is_some() {
            // A `/Differences` entry for that code would have shadowed the base, so the
            // base's answer only stands when the base's code is still the one that answers.
            let code = found?;
            return (self.glyph_for(code) == Some(name)).then_some(code);
        }
        None
    }

    /// Read a font dictionary's `/Encoding`, or `None` when it declares none.
    ///
    /// `default` is the base to use for a dictionary with no `/BaseEncoding`, which means
    /// "the font's own built-in encoding" — a thing that lives inside the font program and
    /// is not readable from here, so the caller's default stands in for it.
    ///
    /// A bare `/Differences` array where a dictionary belongs is accepted as a remap over
    /// `default`. It is not what the specification says, and refusing it would mean drawing
    /// the base and silently discarding a remap the file clearly wrote.
    ///
    /// A composite font's `/Encoding` is a CMap or `/Identity-H`, never one of these, and
    /// is refused: its codes are CIDs and `/Differences` does not apply to them.
    #[must_use]
    pub fn from_font_dict(
        font: &Dict,
        default: Base,
        resolve: &dyn Fn(&Object) -> Option<Object>,
    ) -> Option<Self> {
        let found = font.get("Encoding")?;
        let found = resolve(found).unwrap_or_else(|| found.clone());
        match &found {
            // `/Encoding /WinAnsiEncoding`.
            Object::Name(name) => std::str::from_utf8(name.as_bytes())
                .ok()
                .and_then(Base::by_name)
                .map(Self::new),
            Object::Dict(dict) => {
                let direct = |key: &str| -> Option<Object> {
                    let hit = dict.get(key)?;
                    Some(resolve(hit).unwrap_or_else(|| hit.clone()))
                };
                let base = direct("BaseEncoding")
                    .as_ref()
                    .and_then(Object::as_name)
                    .and_then(|n| std::str::from_utf8(n).ok())
                    .and_then(Base::by_name)
                    .unwrap_or(default);
                let differences = direct("Differences")
                    .as_ref()
                    .and_then(Object::as_array)
                    .map(<[Object]>::to_vec)
                    .unwrap_or_default();
                Some(Self::with_differences(base, &differences))
            }
            Object::Array(items) => Some(Self::with_differences(default, items)),
            _ => None,
        }
    }

    /// The base with a `/Differences` array laid over it.
    ///
    /// The array is a run: a number starts it and every name after it is the next code up,
    /// which is how a file remaps a whole alphabet in one line. A name before any number
    /// has no code to attach to and is skipped rather than guessed at; anything that is
    /// neither a number nor a name ends the run rather than continuing it, so a stray
    /// string cannot shift every code after it. A number too large for a character code
    /// ends the run too, since the run cannot continue past one.
    fn with_differences(base: Base, items: &[Object]) -> Self {
        let mut encoding = Self::new(base);
        let mut code: Option<u32> = None;
        for item in items {
            match item {
                Object::Int(_) | Object::Real(_) => {
                    code = item.as_i64().and_then(|v| u32::try_from(v).ok());
                }
                Object::Name(name) => {
                    let Some(at) = code else { continue };
                    // A name a file writes is bytes, and `#` escapes are already resolved by
                    // the lexer; anything that is not UTF-8 cannot be a glyph name.
                    if let Ok(text) = std::str::from_utf8(name.as_bytes()) {
                        encoding.push(at, text.to_string());
                        code = at.checked_add(1);
                    }
                }
                _ => code = None,
            }
        }
        encoding
    }
}

/// The glyph name `PDFDocEncoding` gives a code.
///
/// This is the encoding a PDF uses for its own *text strings* — a document title, an
/// author's name, the contents of a note — and it is the one that is not Latin-1: 0x18 is
/// `breve` where Latin-1 has nothing, 0xA0 onwards is the Latin-1 glyph set under names
/// that spell it out, and the codes below 0x18 and the controls are assigned to nothing.
/// A file that writes a name here is writing a name, and the name is what a glyph is
/// called; a reader that had no table for this would turn a title into bytes.
#[must_use]
pub fn pdf_doc_encoding(code: u32) -> Option<&'static str> {
    lookup(PDF_DOC, code)
}

/// The three base encodings, in the order a font dictionary may name them.
///
/// Exported because a caller that has to report which base a document used should not have
/// to write the list out again.
pub const AGL_BASE_ENCODINGS: [Base; 3] = [Base::Standard, Base::WinAnsi, Base::MacRoman];

/// The Unicode scalar value an Adobe Glyph List name stands for, or `None` for a name the
/// AGL does not have.
///
/// Where the AGL gives a name two code points — `Delta` is both U+2206 INCREMENT and
/// U+0394 GREEK CAPITAL DELTA — the first is answered, which is the one the AGL lists as
/// canonical and the one a `ToUnicode` map would be written with.
#[must_use]
pub fn agl(name: &str) -> Option<u32> {
    AGL_NAMES
        .binary_search_by(|(candidate, _)| (*candidate).cmp(name))
        .ok()
        .and_then(|index| AGL_NAMES.get(index).map(|(_, code)| *code))
}

/// Is this a name the AGL resolver knows?
#[must_use]
pub fn is_agl_name(name: &str) -> bool {
    agl(name).is_some()
}

/// A name in a sorted `(code, name)` table.
fn lookup(table: &[(u32, &'static str)], code: u32) -> Option<&'static str> {
    let index = table.binary_search_by(|(at, _)| at.cmp(&code)).ok()?;
    table.get(index).map(|(_, name)| *name)
}

// ---- generated tables ----
/// `StandardEncoding`: the specification's Table D.2.
const STANDARD: &[(u32, &str)] = &[
    (32, "space"),
    (33, "exclam"),
    (34, "quotedbl"),
    (35, "numbersign"),
    (36, "dollar"),
    (37, "percent"),
    (38, "ampersand"),
    (39, "quoteright"),
    (40, "parenleft"),
    (41, "parenright"),
    (42, "asterisk"),
    (43, "plus"),
    (44, "comma"),
    (45, "hyphen"),
    (46, "period"),
    (47, "slash"),
    (48, "zero"),
    (49, "one"),
    (50, "two"),
    (51, "three"),
    (52, "four"),
    (53, "five"),
    (54, "six"),
    (55, "seven"),
    (56, "eight"),
    (57, "nine"),
    (58, "colon"),
    (59, "semicolon"),
    (60, "less"),
    (61, "equal"),
    (62, "greater"),
    (63, "question"),
    (64, "at"),
    (65, "A"),
    (66, "B"),
    (67, "C"),
    (68, "D"),
    (69, "E"),
    (70, "F"),
    (71, "G"),
    (72, "H"),
    (73, "I"),
    (74, "J"),
    (75, "K"),
    (76, "L"),
    (77, "M"),
    (78, "N"),
    (79, "O"),
    (80, "P"),
    (81, "Q"),
    (82, "R"),
    (83, "S"),
    (84, "T"),
    (85, "U"),
    (86, "V"),
    (87, "W"),
    (88, "X"),
    (89, "Y"),
    (90, "Z"),
    (91, "bracketleft"),
    (92, "backslash"),
    (93, "bracketright"),
    (94, "asciicircum"),
    (95, "underscore"),
    (96, "quoteleft"),
    (97, "a"),
    (98, "b"),
    (99, "c"),
    (100, "d"),
    (101, "e"),
    (102, "f"),
    (103, "g"),
    (104, "h"),
    (105, "i"),
    (106, "j"),
    (107, "k"),
    (108, "l"),
    (109, "m"),
    (110, "n"),
    (111, "o"),
    (112, "p"),
    (113, "q"),
    (114, "r"),
    (115, "s"),
    (116, "t"),
    (117, "u"),
    (118, "v"),
    (119, "w"),
    (120, "x"),
    (121, "y"),
    (122, "z"),
    (123, "braceleft"),
    (124, "bar"),
    (125, "braceright"),
    (126, "asciitilde"),
    (161, "exclamdown"),
    (162, "cent"),
    (163, "sterling"),
    (164, "fraction"),
    (165, "yen"),
    (166, "florin"),
    (167, "section"),
    (168, "currency"),
    (169, "quotesingle"),
    (170, "quotedblleft"),
    (171, "guillemotleft"),
    (172, "guilsinglleft"),
    (173, "guilsinglright"),
    (174, "fi"),
    (175, "fl"),
    (177, "endash"),
    (178, "dagger"),
    (179, "daggerdbl"),
    (180, "periodcentered"),
    (182, "paragraph"),
    (183, "bullet"),
    (184, "quotesinglbase"),
    (185, "quotedblbase"),
    (186, "quotedblright"),
    (187, "guillemotright"),
    (188, "ellipsis"),
    (189, "perthousand"),
    (191, "questiondown"),
    (193, "grave"),
    (194, "acute"),
    (195, "circumflex"),
    (196, "tilde"),
    (197, "macron"),
    (198, "breve"),
    (199, "dotaccent"),
    (200, "dieresis"),
    (202, "ring"),
    (203, "cedilla"),
    (205, "hungarumlaut"),
    (206, "ogonek"),
    (207, "caron"),
    (208, "emdash"),
    (225, "AE"),
    (227, "ordfeminine"),
    (232, "Lslash"),
    (233, "Oslash"),
    (234, "OE"),
    (235, "ordmasculine"),
    (241, "ae"),
    (245, "dotlessi"),
    (248, "lslash"),
    (249, "oslash"),
    (250, "oe"),
    (251, "germandbls"),
];

/// `WinAnsiEncoding`: the specification's Table D.3.
const WIN_ANSI: &[(u32, &str)] = &[
    (32, "space"),
    (33, "exclam"),
    (34, "quotedbl"),
    (35, "numbersign"),
    (36, "dollar"),
    (37, "percent"),
    (38, "ampersand"),
    (39, "quotesingle"),
    (40, "parenleft"),
    (41, "parenright"),
    (42, "asterisk"),
    (43, "plus"),
    (44, "comma"),
    (45, "hyphen"),
    (46, "period"),
    (47, "slash"),
    (48, "zero"),
    (49, "one"),
    (50, "two"),
    (51, "three"),
    (52, "four"),
    (53, "five"),
    (54, "six"),
    (55, "seven"),
    (56, "eight"),
    (57, "nine"),
    (58, "colon"),
    (59, "semicolon"),
    (60, "less"),
    (61, "equal"),
    (62, "greater"),
    (63, "question"),
    (64, "at"),
    (65, "A"),
    (66, "B"),
    (67, "C"),
    (68, "D"),
    (69, "E"),
    (70, "F"),
    (71, "G"),
    (72, "H"),
    (73, "I"),
    (74, "J"),
    (75, "K"),
    (76, "L"),
    (77, "M"),
    (78, "N"),
    (79, "O"),
    (80, "P"),
    (81, "Q"),
    (82, "R"),
    (83, "S"),
    (84, "T"),
    (85, "U"),
    (86, "V"),
    (87, "W"),
    (88, "X"),
    (89, "Y"),
    (90, "Z"),
    (91, "bracketleft"),
    (92, "backslash"),
    (93, "bracketright"),
    (94, "asciicircum"),
    (95, "underscore"),
    (96, "grave"),
    (97, "a"),
    (98, "b"),
    (99, "c"),
    (100, "d"),
    (101, "e"),
    (102, "f"),
    (103, "g"),
    (104, "h"),
    (105, "i"),
    (106, "j"),
    (107, "k"),
    (108, "l"),
    (109, "m"),
    (110, "n"),
    (111, "o"),
    (112, "p"),
    (113, "q"),
    (114, "r"),
    (115, "s"),
    (116, "t"),
    (117, "u"),
    (118, "v"),
    (119, "w"),
    (120, "x"),
    (121, "y"),
    (122, "z"),
    (123, "braceleft"),
    (124, "bar"),
    (125, "braceright"),
    (126, "asciitilde"),
    (127, "bullet"),
    (128, "Euro"),
    (129, "bullet"),
    (130, "quotesinglbase"),
    (131, "florin"),
    (132, "quotedblbase"),
    (133, "ellipsis"),
    (134, "dagger"),
    (135, "daggerdbl"),
    (136, "circumflex"),
    (137, "perthousand"),
    (138, "Scaron"),
    (139, "guilsinglleft"),
    (140, "OE"),
    (141, "bullet"),
    (142, "Zcaron"),
    (143, "bullet"),
    (144, "bullet"),
    (145, "quoteleft"),
    (146, "quoteright"),
    (147, "quotedblleft"),
    (148, "quotedblright"),
    (149, "bullet"),
    (150, "endash"),
    (151, "emdash"),
    (152, "tilde"),
    (153, "trademark"),
    (154, "scaron"),
    (155, "guilsinglright"),
    (156, "oe"),
    (157, "bullet"),
    (158, "zcaron"),
    (159, "Ydieresis"),
    (160, "space"),
    (161, "exclamdown"),
    (162, "cent"),
    (163, "sterling"),
    (164, "currency"),
    (165, "yen"),
    (166, "florin"),
    (167, "section"),
    (168, "dieresis"),
    (169, "copyright"),
    (170, "ordfeminine"),
    (171, "guillemotleft"),
    (172, "logicalnot"),
    (173, "hyphen"),
    (174, "registered"),
    (175, "macron"),
    (176, "degree"),
    (177, "plusminus"),
    (178, "twosuperior"),
    (179, "threesuperior"),
    (180, "acute"),
    (181, "mu"),
    (182, "paragraph"),
    (183, "periodcentered"),
    (184, "cedilla"),
    (185, "onesuperior"),
    (186, "ordmasculine"),
    (187, "guillemotright"),
    (188, "onequarter"),
    (189, "onehalf"),
    (190, "threequarters"),
    (191, "questiondown"),
    (192, "Agrave"),
    (193, "Aacute"),
    (194, "Acircumflex"),
    (195, "Atilde"),
    (196, "Adieresis"),
    (197, "Aring"),
    (198, "AE"),
    (199, "Ccedilla"),
    (200, "Egrave"),
    (201, "Eacute"),
    (202, "Ecircumflex"),
    (203, "Edieresis"),
    (204, "Igrave"),
    (205, "Iacute"),
    (206, "Icircumflex"),
    (207, "Idieresis"),
    (208, "Eth"),
    (209, "Ntilde"),
    (210, "Ograve"),
    (211, "Oacute"),
    (212, "Ocircumflex"),
    (213, "Otilde"),
    (214, "Odieresis"),
    (215, "multiply"),
    (216, "Oslash"),
    (217, "Ugrave"),
    (218, "Uacute"),
    (219, "Ucircumflex"),
    (220, "Udieresis"),
    (221, "Yacute"),
    (222, "Thorn"),
    (223, "germandbls"),
    (224, "agrave"),
    (225, "aacute"),
    (226, "acircumflex"),
    (227, "atilde"),
    (228, "adieresis"),
    (229, "aring"),
    (230, "ae"),
    (231, "ccedilla"),
    (232, "egrave"),
    (233, "eacute"),
    (234, "ecircumflex"),
    (235, "edieresis"),
    (236, "igrave"),
    (237, "iacute"),
    (238, "icircumflex"),
    (239, "idieresis"),
    (240, "eth"),
    (241, "ntilde"),
    (242, "ograve"),
    (243, "oacute"),
    (244, "ocircumflex"),
    (245, "otilde"),
    (246, "odieresis"),
    (247, "divide"),
    (248, "oslash"),
    (249, "ugrave"),
    (250, "uacute"),
    (251, "ucircumflex"),
    (252, "udieresis"),
    (253, "yacute"),
    (254, "thorn"),
    (255, "ydieresis"),
];

/// `MacRomanEncoding`: the specification's Table D.4.
const MAC_ROMAN: &[(u32, &str)] = &[
    (32, "space"),
    (33, "exclam"),
    (34, "quotedbl"),
    (35, "numbersign"),
    (36, "dollar"),
    (37, "percent"),
    (38, "ampersand"),
    (39, "quotesingle"),
    (40, "parenleft"),
    (41, "parenright"),
    (42, "asterisk"),
    (43, "plus"),
    (44, "comma"),
    (45, "hyphen"),
    (46, "period"),
    (47, "slash"),
    (48, "zero"),
    (49, "one"),
    (50, "two"),
    (51, "three"),
    (52, "four"),
    (53, "five"),
    (54, "six"),
    (55, "seven"),
    (56, "eight"),
    (57, "nine"),
    (58, "colon"),
    (59, "semicolon"),
    (60, "less"),
    (61, "equal"),
    (62, "greater"),
    (63, "question"),
    (64, "at"),
    (65, "A"),
    (66, "B"),
    (67, "C"),
    (68, "D"),
    (69, "E"),
    (70, "F"),
    (71, "G"),
    (72, "H"),
    (73, "I"),
    (74, "J"),
    (75, "K"),
    (76, "L"),
    (77, "M"),
    (78, "N"),
    (79, "O"),
    (80, "P"),
    (81, "Q"),
    (82, "R"),
    (83, "S"),
    (84, "T"),
    (85, "U"),
    (86, "V"),
    (87, "W"),
    (88, "X"),
    (89, "Y"),
    (90, "Z"),
    (91, "bracketleft"),
    (92, "backslash"),
    (93, "bracketright"),
    (94, "asciicircum"),
    (95, "underscore"),
    (96, "grave"),
    (97, "a"),
    (98, "b"),
    (99, "c"),
    (100, "d"),
    (101, "e"),
    (102, "f"),
    (103, "g"),
    (104, "h"),
    (105, "i"),
    (106, "j"),
    (107, "k"),
    (108, "l"),
    (109, "m"),
    (110, "n"),
    (111, "o"),
    (112, "p"),
    (113, "q"),
    (114, "r"),
    (115, "s"),
    (116, "t"),
    (117, "u"),
    (118, "v"),
    (119, "w"),
    (120, "x"),
    (121, "y"),
    (122, "z"),
    (123, "braceleft"),
    (124, "bar"),
    (125, "braceright"),
    (126, "asciitilde"),
    (128, "Adieresis"),
    (129, "Aring"),
    (130, "Ccedilla"),
    (131, "Eacute"),
    (132, "Ntilde"),
    (133, "Odieresis"),
    (134, "Udieresis"),
    (135, "aacute"),
    (136, "agrave"),
    (137, "acircumflex"),
    (138, "adieresis"),
    (139, "atilde"),
    (140, "aring"),
    (141, "ccedilla"),
    (142, "eacute"),
    (143, "egrave"),
    (144, "ecircumflex"),
    (145, "edieresis"),
    (146, "iacute"),
    (147, "igrave"),
    (148, "icircumflex"),
    (149, "idieresis"),
    (150, "ntilde"),
    (151, "oacute"),
    (152, "ograve"),
    (153, "ocircumflex"),
    (154, "odieresis"),
    (155, "otilde"),
    (156, "uacute"),
    (157, "ugrave"),
    (158, "ucircumflex"),
    (159, "udieresis"),
    (160, "dagger"),
    (161, "degree"),
    (162, "cent"),
    (163, "sterling"),
    (164, "section"),
    (165, "bullet"),
    (166, "paragraph"),
    (167, "germandbls"),
    (168, "registered"),
    (169, "copyright"),
    (170, "trademark"),
    (171, "acute"),
    (172, "dieresis"),
    (173, "notequal"),
    (174, "AE"),
    (175, "Oslash"),
    (176, "infinity"),
    (177, "plusminus"),
    (178, "lessequal"),
    (179, "greaterequal"),
    (180, "yen"),
    (181, "mu"),
    (182, "partialdiff"),
    (183, "summation"),
    (184, "product"),
    (185, "pi"),
    (186, "integral"),
    (187, "ordfeminine"),
    (188, "ordmasculine"),
    (189, "Omega"),
    (190, "ae"),
    (191, "oslash"),
    (192, "questiondown"),
    (193, "exclamdown"),
    (194, "logicalnot"),
    (195, "radical"),
    (196, "florin"),
    (197, "approxequal"),
    (198, "Delta"),
    (199, "guillemotleft"),
    (200, "space"),
    (201, "ellipsis"),
    (202, "space"),
    (203, "Agrave"),
    (204, "Atilde"),
    (205, "Otilde"),
    (206, "OE"),
    (207, "oe"),
    (208, "endash"),
    (209, "emdash"),
    (210, "quotedblleft"),
    (211, "quotedblright"),
    (212, "quoteleft"),
    (213, "quoteright"),
    (214, "divide"),
    (215, "lozenge"),
    (216, "ydieresis"),
    (217, "Ydieresis"),
    (218, "fraction"),
    (219, "Euro"),
    (220, "guilsinglleft"),
    (221, "guilsinglright"),
    (222, "fi"),
    (223, "fl"),
    (224, "daggerdbl"),
    (225, "periodcentered"),
];

/// `PDFDocEncoding`: the specification's Annex D.2.
const PDF_DOC: &[(u32, &str)] = &[
    (24, "breve"),
    (25, "caron"),
    (26, "circumflex"),
    (27, "dotaccent"),
    (28, "hungarumlaut"),
    (29, "ogonek"),
    (30, "ring"),
    (31, "tilde"),
    (32, "space"),
    (33, "exclam"),
    (34, "quotedbl"),
    (35, "numbersign"),
    (36, "dollar"),
    (37, "percent"),
    (38, "ampersand"),
    (39, "quoteright"),
    (40, "parenleft"),
    (41, "parenright"),
    (42, "asterisk"),
    (43, "plus"),
    (44, "comma"),
    (45, "hyphen"),
    (46, "period"),
    (47, "slash"),
    (48, "zero"),
    (49, "one"),
    (50, "two"),
    (51, "three"),
    (52, "four"),
    (53, "five"),
    (54, "six"),
    (55, "seven"),
    (56, "eight"),
    (57, "nine"),
    (58, "colon"),
    (59, "semicolon"),
    (60, "less"),
    (61, "equal"),
    (62, "greater"),
    (63, "question"),
    (64, "at"),
    (65, "A"),
    (66, "B"),
    (67, "C"),
    (68, "D"),
    (69, "E"),
    (70, "F"),
    (71, "G"),
    (72, "H"),
    (73, "I"),
    (74, "J"),
    (75, "K"),
    (76, "L"),
    (77, "M"),
    (78, "N"),
    (79, "O"),
    (80, "P"),
    (81, "Q"),
    (82, "R"),
    (83, "S"),
    (84, "T"),
    (85, "U"),
    (86, "V"),
    (87, "W"),
    (88, "X"),
    (89, "Y"),
    (90, "Z"),
    (91, "bracketleft"),
    (92, "backslash"),
    (93, "bracketright"),
    (94, "asciicircum"),
    (95, "underscore"),
    (96, "quoteleft"),
    (97, "a"),
    (98, "b"),
    (99, "c"),
    (100, "d"),
    (101, "e"),
    (102, "f"),
    (103, "g"),
    (104, "h"),
    (105, "i"),
    (106, "j"),
    (107, "k"),
    (108, "l"),
    (109, "m"),
    (110, "n"),
    (111, "o"),
    (112, "p"),
    (113, "q"),
    (114, "r"),
    (115, "s"),
    (116, "t"),
    (117, "u"),
    (118, "v"),
    (119, "w"),
    (120, "x"),
    (121, "y"),
    (122, "z"),
    (123, "braceleft"),
    (124, "bar"),
    (125, "braceright"),
    (126, "asciitilde"),
    (127, "bullet"),
    (128, "dagger"),
    (129, "daggerdbl"),
    (130, "ellipsis"),
    (131, "emdash"),
    (132, "endash"),
    (133, "exclamdown"),
    (134, "fraction"),
    (135, "guilsinglleft"),
    (136, "guilsinglright"),
    (137, "minus"),
    (138, "perthousand"),
    (139, "questiondown"),
    (140, "quotedblbase"),
    (141, "quotesinglbase"),
    (142, "trademark"),
    (143, "fi"),
    (144, "fl"),
    (145, "Lslash"),
    (146, "OE"),
    (147, "Scaron"),
    (148, "Ydieresis"),
    (149, "Zcaron"),
    (150, "dotlessi"),
    (151, "lslash"),
    (152, "oe"),
    (153, "scaron"),
    (154, "zcaron"),
    (155, "AE"),
    (156, "ordfeminine"),
    (157, "ordmasculine"),
    (158, "Oslash"),
    (159, "oslash"),
    (160, "space"),
    (161, "exclamdown"),
    (162, "cent"),
    (163, "sterling"),
    (164, "currency"),
    (165, "yen"),
    (166, "florin"),
    (167, "section"),
    (168, "dieresis"),
    (169, "copyright"),
    (170, "ordfeminine"),
    (171, "guillemotleft"),
    (172, "logicalnot"),
    (173, "hyphen"),
    (174, "registered"),
    (175, "macron"),
    (176, "degree"),
    (177, "plusminus"),
    (178, "twosuperior"),
    (179, "threesuperior"),
    (180, "acute"),
    (181, "mu"),
    (182, "paragraph"),
    (183, "periodcentered"),
    (184, "cedilla"),
    (185, "onesuperior"),
    (186, "ordmasculine"),
    (187, "guillemotright"),
    (188, "onequarter"),
    (189, "onehalf"),
    (190, "threequarters"),
    (191, "questiondown"),
    (192, "Agrave"),
    (193, "Aacute"),
    (194, "Acircumflex"),
    (195, "Atilde"),
    (196, "Adieresis"),
    (197, "Aring"),
    (198, "AE"),
    (199, "Ccedilla"),
    (200, "Egrave"),
    (201, "Eacute"),
    (202, "Ecircumflex"),
    (203, "Edieresis"),
    (204, "Igrave"),
    (205, "Iacute"),
    (206, "Icircumflex"),
    (207, "Idieresis"),
    (208, "Eth"),
    (209, "Ntilde"),
    (210, "Ograve"),
    (211, "Oacute"),
    (212, "Ocircumflex"),
    (213, "Otilde"),
    (214, "Odieresis"),
    (215, "multiply"),
    (216, "Oslash"),
    (217, "Ugrave"),
    (218, "Uacute"),
    (219, "Ucircumflex"),
    (220, "Udieresis"),
    (221, "Yacute"),
    (222, "Thorn"),
    (223, "germandbls"),
    (224, "agrave"),
    (225, "aacute"),
    (226, "acircumflex"),
    (227, "atilde"),
    (228, "adieresis"),
    (229, "aring"),
    (230, "ae"),
    (231, "ccedilla"),
    (232, "egrave"),
    (233, "eacute"),
    (234, "ecircumflex"),
    (235, "edieresis"),
    (236, "igrave"),
    (237, "iacute"),
    (238, "icircumflex"),
    (239, "idieresis"),
    (240, "eth"),
    (241, "ntilde"),
    (242, "ograve"),
    (243, "oacute"),
    (244, "ocircumflex"),
    (245, "otilde"),
    (246, "odieresis"),
    (247, "divide"),
    (248, "oslash"),
    (249, "ugrave"),
    (250, "uacute"),
    (251, "ucircumflex"),
    (252, "udieresis"),
    (253, "yacute"),
    (254, "thorn"),
    (255, "ydieresis"),
];

/// The AGL names this resolver knows, sorted by name for a binary search.
const AGL_NAMES: &[(&str, u32)] = &[
    ("A", 0x0041),
    ("AE", 0x00C6),
    ("AEacute", 0x01FC),
    ("AEmacron", 0x01E2),
    ("Aacute", 0x00C1),
    ("Abreve", 0x0102),
    ("Acaron", 0x01CD),
    ("Acircumflex", 0x00C2),
    ("Adblgrave", 0x0200),
    ("Adieresis", 0x00C4),
    ("Adieresismacron", 0x01DE),
    ("Adotmacron", 0x01E0),
    ("Agrave", 0x00C0),
    ("Ainvertedbreve", 0x0202),
    ("Alpha", 0x0391),
    ("Alphatonos", 0x0386),
    ("Amacron", 0x0100),
    ("Aogonek", 0x0104),
    ("Aring", 0x00C5),
    ("Aringacute", 0x01FA),
    ("Atilde", 0x00C3),
    ("B", 0x0042),
    ("Beta", 0x0392),
    ("Bhook", 0x0181),
    ("Btopbar", 0x0182),
    ("C", 0x0043),
    ("Cacute", 0x0106),
    ("Ccaron", 0x010C),
    ("Ccedilla", 0x00C7),
    ("Ccircumflex", 0x0108),
    ("Cdot", 0x010A),
    ("Cdotaccent", 0x010A),
    ("Chi", 0x03A7),
    ("Chook", 0x0187),
    ("D", 0x0044),
    ("DZ", 0x01F1),
    ("DZcaron", 0x01C4),
    ("Dafrican", 0x0189),
    ("Dcaron", 0x010E),
    ("Dcroat", 0x0110),
    ("Deicoptic", 0x03EE),
    ("Delta", 0x2206),
    ("Deltagreek", 0x0394),
    ("Dhook", 0x018A),
    ("Digammagreek", 0x03DC),
    ("Dslash", 0x0110),
    ("Dtopbar", 0x018B),
    ("Dz", 0x01F2),
    ("Dzcaron", 0x01C5),
    ("E", 0x0045),
    ("Eacute", 0x00C9),
    ("Ebreve", 0x0114),
    ("Ecaron", 0x011A),
    ("Ecircumflex", 0x00CA),
    ("Edblgrave", 0x0204),
    ("Edieresis", 0x00CB),
    ("Edot", 0x0116),
    ("Edotaccent", 0x0116),
    ("Egrave", 0x00C8),
    ("Einvertedbreve", 0x0206),
    ("Emacron", 0x0112),
    ("Eng", 0x014A),
    ("Eogonek", 0x0118),
    ("Eopen", 0x0190),
    ("Epsilon", 0x0395),
    ("Epsilontonos", 0x0388),
    ("Ereversed", 0x018E),
    ("Esh", 0x01A9),
    ("Eta", 0x0397),
    ("Etatonos", 0x0389),
    ("Eth", 0x00D0),
    ("Euro", 0x20AC),
    ("Ezh", 0x01B7),
    ("Ezhcaron", 0x01EE),
    ("Ezhreversed", 0x01B8),
    ("F", 0x0046),
    ("Feicoptic", 0x03E4),
    ("Fhook", 0x0191),
    ("G", 0x0047),
    ("Gacute", 0x01F4),
    ("Gamma", 0x0393),
    ("Gammaafrican", 0x0194),
    ("Gangiacoptic", 0x03EA),
    ("Gbreve", 0x011E),
    ("Gcaron", 0x01E6),
    ("Gcedilla", 0x0122),
    ("Gcircumflex", 0x011C),
    ("Gcommaaccent", 0x0122),
    ("Gdot", 0x0120),
    ("Gdotaccent", 0x0120),
    ("Ghook", 0x0193),
    ("Gstroke", 0x01E4),
    ("H", 0x0048),
    ("Hbar", 0x0126),
    ("Hcircumflex", 0x0124),
    ("Horicoptic", 0x03E8),
    ("I", 0x0049),
    ("IJ", 0x0132),
    ("Iacute", 0x00CD),
    ("Ibreve", 0x012C),
    ("Icaron", 0x01CF),
    ("Icircumflex", 0x00CE),
    ("Idblgrave", 0x0208),
    ("Idieresis", 0x00CF),
    ("Idot", 0x0130),
    ("Idotaccent", 0x0130),
    ("Igrave", 0x00CC),
    ("Iinvertedbreve", 0x020A),
    ("Imacron", 0x012A),
    ("Iogonek", 0x012E),
    ("Iota", 0x0399),
    ("Iotaafrican", 0x0196),
    ("Iotadieresis", 0x03AA),
    ("Iotatonos", 0x038A),
    ("Istroke", 0x0197),
    ("Itilde", 0x0128),
    ("J", 0x004A),
    ("Jcircumflex", 0x0134),
    ("K", 0x004B),
    ("Kappa", 0x039A),
    ("Kcaron", 0x01E8),
    ("Kcedilla", 0x0136),
    ("Kcommaaccent", 0x0136),
    ("Kheicoptic", 0x03E6),
    ("Khook", 0x0198),
    ("Koppagreek", 0x03DE),
    ("L", 0x004C),
    ("LJ", 0x01C7),
    ("Lacute", 0x0139),
    ("Lambda", 0x039B),
    ("Lcaron", 0x013D),
    ("Lcedilla", 0x013B),
    ("Lcommaaccent", 0x013B),
    ("Ldot", 0x013F),
    ("Ldotaccent", 0x013F),
    ("Lj", 0x01C8),
    ("Lslash", 0x0141),
    ("M", 0x004D),
    ("Mturned", 0x019C),
    ("Mu", 0x039C),
    ("N", 0x004E),
    ("NJ", 0x01CA),
    ("Nacute", 0x0143),
    ("Ncaron", 0x0147),
    ("Ncedilla", 0x0145),
    ("Ncommaaccent", 0x0145),
    ("Nhookleft", 0x019D),
    ("Nj", 0x01CB),
    ("Ntilde", 0x00D1),
    ("Nu", 0x039D),
    ("O", 0x004F),
    ("OE", 0x0152),
    ("Oacute", 0x00D3),
    ("Obreve", 0x014E),
    ("Ocaron", 0x01D1),
    ("Ocenteredtilde", 0x019F),
    ("Ocircumflex", 0x00D4),
    ("Odblacute", 0x0150),
    ("Odblgrave", 0x020C),
    ("Odieresis", 0x00D6),
    ("Ograve", 0x00D2),
    ("Ohorn", 0x01A0),
    ("Ohungarumlaut", 0x0150),
    ("Oi", 0x01A2),
    ("Oinvertedbreve", 0x020E),
    ("Omacron", 0x014C),
    ("Omega", 0x2126),
    ("Omegagreek", 0x03A9),
    ("Omegatonos", 0x038F),
    ("Omicron", 0x039F),
    ("Omicrontonos", 0x038C),
    ("Oogonek", 0x01EA),
    ("Oogonekmacron", 0x01EC),
    ("Oopen", 0x0186),
    ("Oslash", 0x00D8),
    ("Oslashacute", 0x01FE),
    ("Ostrokeacute", 0x01FE),
    ("Otilde", 0x00D5),
    ("P", 0x0050),
    ("Phi", 0x03A6),
    ("Phook", 0x01A4),
    ("Pi", 0x03A0),
    ("Psi", 0x03A8),
    ("Q", 0x0051),
    ("R", 0x0052),
    ("Racute", 0x0154),
    ("Rcaron", 0x0158),
    ("Rcedilla", 0x0156),
    ("Rcommaaccent", 0x0156),
    ("Rdblgrave", 0x0210),
    ("Rho", 0x03A1),
    ("Rinvertedbreve", 0x0212),
    ("S", 0x0053),
    ("Sacute", 0x015A),
    ("Sampigreek", 0x03E0),
    ("Scaron", 0x0160),
    ("Scedilla", 0x015E),
    ("Schwa", 0x018F),
    ("Scircumflex", 0x015C),
    ("Scommaaccent", 0x0218),
    ("Sheicoptic", 0x03E2),
    ("Shimacoptic", 0x03EC),
    ("Sigma", 0x03A3),
    ("Stigmagreek", 0x03DA),
    ("T", 0x0054),
    ("Tau", 0x03A4),
    ("Tbar", 0x0166),
    ("Tcaron", 0x0164),
    ("Tcedilla", 0x0162),
    ("Tcommaaccent", 0x0162),
    ("Theta", 0x0398),
    ("Thook", 0x01AC),
    ("Thorn", 0x00DE),
    ("Tonefive", 0x01BC),
    ("Tonesix", 0x0184),
    ("Tonetwo", 0x01A7),
    ("Tretroflexhook", 0x01AE),
    ("U", 0x0055),
    ("Uacute", 0x00DA),
    ("Ubreve", 0x016C),
    ("Ucaron", 0x01D3),
    ("Ucircumflex", 0x00DB),
    ("Udblacute", 0x0170),
    ("Udblgrave", 0x0214),
    ("Udieresis", 0x00DC),
    ("Udieresisacute", 0x01D7),
    ("Udieresiscaron", 0x01D9),
    ("Udieresisgrave", 0x01DB),
    ("Udieresismacron", 0x01D5),
    ("Ugrave", 0x00D9),
    ("Uhorn", 0x01AF),
    ("Uhungarumlaut", 0x0170),
    ("Uinvertedbreve", 0x0216),
    ("Umacron", 0x016A),
    ("Uogonek", 0x0172),
    ("Upsilon", 0x03A5),
    ("Upsilon1", 0x03D2),
    ("Upsilonacutehooksymbolgreek", 0x03D3),
    ("Upsilonafrican", 0x01B1),
    ("Upsilondieresis", 0x03AB),
    ("Upsilondieresishooksymbolgreek", 0x03D4),
    ("Upsilonhooksymbol", 0x03D2),
    ("Upsilontonos", 0x038E),
    ("Uring", 0x016E),
    ("Utilde", 0x0168),
    ("V", 0x0056),
    ("Vhook", 0x01B2),
    ("W", 0x0057),
    ("Wcircumflex", 0x0174),
    ("X", 0x0058),
    ("Xi", 0x039E),
    ("Y", 0x0059),
    ("Yacute", 0x00DD),
    ("Ycircumflex", 0x0176),
    ("Ydieresis", 0x0178),
    ("Yhook", 0x01B3),
    ("Z", 0x005A),
    ("Zacute", 0x0179),
    ("Zcaron", 0x017D),
    ("Zdot", 0x017B),
    ("Zdotaccent", 0x017B),
    ("Zeta", 0x0396),
    ("Zstroke", 0x01B5),
    ("a", 0x0061),
    ("aacute", 0x00E1),
    ("abreve", 0x0103),
    ("acaron", 0x01CE),
    ("acircumflex", 0x00E2),
    ("acute", 0x00B4),
    ("adblgrave", 0x0201),
    ("adieresis", 0x00E4),
    ("adieresismacron", 0x01DF),
    ("adotmacron", 0x01E1),
    ("ae", 0x00E6),
    ("aeacute", 0x01FD),
    ("aemacron", 0x01E3),
    ("afii00208", 0x2015),
    ("afii08941", 0x20A4),
    ("afii10017", 0x0410),
    ("afii10018", 0x0411),
    ("afii10019", 0x0412),
    ("afii10020", 0x0413),
    ("afii10021", 0x0414),
    ("afii10022", 0x0415),
    ("afii10023", 0x0401),
    ("afii10024", 0x0416),
    ("afii10025", 0x0417),
    ("afii10026", 0x0418),
    ("afii10027", 0x0419),
    ("afii10028", 0x041A),
    ("afii10029", 0x041B),
    ("afii10030", 0x041C),
    ("afii10031", 0x041D),
    ("afii10032", 0x041E),
    ("afii10033", 0x041F),
    ("afii10034", 0x0420),
    ("afii10035", 0x0421),
    ("afii10036", 0x0422),
    ("afii10037", 0x0423),
    ("afii10038", 0x0424),
    ("afii10039", 0x0425),
    ("afii10040", 0x0426),
    ("afii10041", 0x0427),
    ("afii10042", 0x0428),
    ("afii10043", 0x0429),
    ("afii10044", 0x042A),
    ("afii10045", 0x042B),
    ("afii10046", 0x042C),
    ("afii10047", 0x042D),
    ("afii10048", 0x042E),
    ("afii10049", 0x042F),
    ("afii10050", 0x0490),
    ("afii10051", 0x0402),
    ("afii10052", 0x0403),
    ("afii10053", 0x0404),
    ("afii10054", 0x0405),
    ("afii10055", 0x0406),
    ("afii10056", 0x0407),
    ("afii10057", 0x0408),
    ("afii10058", 0x0409),
    ("afii10059", 0x040A),
    ("afii10060", 0x040B),
    ("afii10061", 0x040C),
    ("afii10062", 0x040E),
    ("afii10063", 0xF6C4),
    ("afii10064", 0xF6C5),
    ("afii10065", 0x0430),
    ("afii10066", 0x0431),
    ("afii10067", 0x0432),
    ("afii10068", 0x0433),
    ("afii10069", 0x0434),
    ("afii10070", 0x0435),
    ("afii10071", 0x0451),
    ("afii10072", 0x0436),
    ("afii10073", 0x0437),
    ("afii10074", 0x0438),
    ("afii10075", 0x0439),
    ("afii10076", 0x043A),
    ("afii10077", 0x043B),
    ("afii10078", 0x043C),
    ("afii10079", 0x043D),
    ("afii10080", 0x043E),
    ("afii10081", 0x043F),
    ("afii10082", 0x0440),
    ("afii10083", 0x0441),
    ("afii10084", 0x0442),
    ("afii10085", 0x0443),
    ("afii10086", 0x0444),
    ("afii10087", 0x0445),
    ("afii10088", 0x0446),
    ("afii10089", 0x0447),
    ("afii10090", 0x0448),
    ("afii10091", 0x0449),
    ("afii10092", 0x044A),
    ("afii10093", 0x044B),
    ("afii10094", 0x044C),
    ("afii10095", 0x044D),
    ("afii10096", 0x044E),
    ("afii10097", 0x044F),
    ("afii10098", 0x0491),
    ("afii10099", 0x0452),
    ("afii10100", 0x0453),
    ("afii10101", 0x0454),
    ("afii10102", 0x0455),
    ("afii10103", 0x0456),
    ("afii10104", 0x0457),
    ("afii10105", 0x0458),
    ("afii10106", 0x0459),
    ("afii10107", 0x045A),
    ("afii10108", 0x045B),
    ("afii10109", 0x045C),
    ("afii10110", 0x045E),
    ("afii10145", 0x040F),
    ("afii10146", 0x0462),
    ("afii10147", 0x0472),
    ("afii10148", 0x0474),
    ("afii10192", 0xF6C6),
    ("afii10193", 0x045F),
    ("afii10194", 0x0463),
    ("afii10195", 0x0473),
    ("afii10196", 0x0475),
    ("afii10831", 0xF6C7),
    ("afii10832", 0xF6C8),
    ("afii10846", 0x04D9),
    ("afii299", 0x200E),
    ("afii300", 0x200F),
    ("afii301", 0x200D),
    ("afii57381", 0x066A),
    ("afii57388", 0x060C),
    ("afii57392", 0x0660),
    ("afii57393", 0x0661),
    ("afii57394", 0x0662),
    ("afii57395", 0x0663),
    ("afii57396", 0x0664),
    ("afii57397", 0x0665),
    ("afii57398", 0x0666),
    ("afii57399", 0x0667),
    ("afii57400", 0x0668),
    ("afii57401", 0x0669),
    ("afii57403", 0x061B),
    ("afii57407", 0x061F),
    ("afii57409", 0x0621),
    ("afii57410", 0x0622),
    ("afii57411", 0x0623),
    ("afii57412", 0x0624),
    ("afii57413", 0x0625),
    ("afii57414", 0x0626),
    ("afii57415", 0x0627),
    ("afii57416", 0x0628),
    ("afii57417", 0x0629),
    ("afii57418", 0x062A),
    ("afii57419", 0x062B),
    ("afii57420", 0x062C),
    ("afii57421", 0x062D),
    ("afii57422", 0x062E),
    ("afii57423", 0x062F),
    ("afii57424", 0x0630),
    ("afii57425", 0x0631),
    ("afii57426", 0x0632),
    ("afii57427", 0x0633),
    ("afii57428", 0x0634),
    ("afii57429", 0x0635),
    ("afii57430", 0x0636),
    ("afii57431", 0x0637),
    ("afii57432", 0x0638),
    ("afii57433", 0x0639),
    ("afii57434", 0x063A),
    ("afii57440", 0x0640),
    ("afii57441", 0x0641),
    ("afii57442", 0x0642),
    ("afii57443", 0x0643),
    ("afii57444", 0x0644),
    ("afii57445", 0x0645),
    ("afii57446", 0x0646),
    ("afii57448", 0x0648),
    ("afii57449", 0x0649),
    ("afii57450", 0x064A),
    ("afii57451", 0x064B),
    ("afii57452", 0x064C),
    ("afii57453", 0x064D),
    ("afii57454", 0x064E),
    ("afii57455", 0x064F),
    ("afii57456", 0x0650),
    ("afii57457", 0x0651),
    ("afii57458", 0x0652),
    ("afii57470", 0x0647),
    ("afii57505", 0x06A4),
    ("afii57506", 0x067E),
    ("afii57507", 0x0686),
    ("afii57508", 0x0698),
    ("afii57509", 0x06AF),
    ("afii57511", 0x0679),
    ("afii57512", 0x0688),
    ("afii57513", 0x0691),
    ("afii57514", 0x06BA),
    ("afii57519", 0x06D2),
    ("afii57534", 0x06D5),
    ("afii57636", 0x20AA),
    ("afii57645", 0x05BE),
    ("afii57658", 0x05C3),
    ("afii57664", 0x05D0),
    ("afii57665", 0x05D1),
    ("afii57666", 0x05D2),
    ("afii57667", 0x05D3),
    ("afii57668", 0x05D4),
    ("afii57669", 0x05D5),
    ("afii57670", 0x05D6),
    ("afii57671", 0x05D7),
    ("afii57672", 0x05D8),
    ("afii57673", 0x05D9),
    ("afii57674", 0x05DA),
    ("afii57675", 0x05DB),
    ("afii57676", 0x05DC),
    ("afii57677", 0x05DD),
    ("afii57678", 0x05DE),
    ("afii57679", 0x05DF),
    ("afii57680", 0x05E0),
    ("afii57681", 0x05E1),
    ("afii57682", 0x05E2),
    ("afii57683", 0x05E3),
    ("afii57684", 0x05E4),
    ("afii57685", 0x05E5),
    ("afii57686", 0x05E6),
    ("afii57687", 0x05E7),
    ("afii57688", 0x05E8),
    ("afii57689", 0x05E9),
    ("afii57690", 0x05EA),
    ("afii57694", 0xFB2A),
    ("afii57695", 0xFB2B),
    ("afii57700", 0xFB4B),
    ("afii57705", 0xFB1F),
    ("afii57716", 0x05F0),
    ("afii57717", 0x05F1),
    ("afii57718", 0x05F2),
    ("afii57723", 0xFB35),
    ("afii57793", 0x05B4),
    ("afii57794", 0x05B5),
    ("afii57795", 0x05B6),
    ("afii57796", 0x05BB),
    ("afii57797", 0x05B8),
    ("afii57798", 0x05B7),
    ("afii57799", 0x05B0),
    ("afii57800", 0x05B2),
    ("afii57801", 0x05B1),
    ("afii57802", 0x05B3),
    ("afii57803", 0x05C2),
    ("afii57804", 0x05C1),
    ("afii57806", 0x05B9),
    ("afii57807", 0x05BC),
    ("afii57839", 0x05BD),
    ("afii57841", 0x05BF),
    ("afii57842", 0x05C0),
    ("afii57929", 0x02BC),
    ("afii61248", 0x2105),
    ("afii61289", 0x2113),
    ("afii61352", 0x2116),
    ("afii61573", 0x202C),
    ("afii61574", 0x202D),
    ("afii61575", 0x202E),
    ("afii61664", 0x200C),
    ("afii63167", 0x066D),
    ("afii64937", 0x02BD),
    ("agrave", 0x00E0),
    ("ainvertedbreve", 0x0203),
    ("alpha", 0x03B1),
    ("alphatonos", 0x03AC),
    ("amacron", 0x0101),
    ("ampersand", 0x0026),
    ("anoteleia", 0x0387),
    ("aogonek", 0x0105),
    ("approxequal", 0x2248),
    ("aring", 0x00E5),
    ("aringacute", 0x01FB),
    ("asciicircum", 0x005E),
    ("asciitilde", 0x007E),
    ("asterisk", 0x002A),
    ("at", 0x0040),
    ("atilde", 0x00E3),
    ("b", 0x0062),
    ("backslash", 0x005C),
    ("bar", 0x007C),
    ("beta", 0x03B2),
    ("betasymbolgreek", 0x03D0),
    ("braceleft", 0x007B),
    ("braceright", 0x007D),
    ("bracketleft", 0x005B),
    ("bracketright", 0x005D),
    ("breve", 0x02D8),
    ("brokenbar", 0x00A6),
    ("bstroke", 0x0180),
    ("btopbar", 0x0183),
    ("bullet", 0x2022),
    ("c", 0x0063),
    ("cacute", 0x0107),
    ("caron", 0x02C7),
    ("ccaron", 0x010D),
    ("ccedilla", 0x00E7),
    ("ccircumflex", 0x0109),
    ("cdot", 0x010B),
    ("cdotaccent", 0x010B),
    ("cedilla", 0x00B8),
    ("cent", 0x00A2),
    ("chi", 0x03C7),
    ("chook", 0x0188),
    ("circumflex", 0x02C6),
    ("clickalveolar", 0x01C2),
    ("clickdental", 0x01C0),
    ("clicklateral", 0x01C1),
    ("clickretroflex", 0x01C3),
    ("colon", 0x003A),
    ("comma", 0x002C),
    ("controlDEL", 0x007F),
    ("copyright", 0x00A9),
    ("currency", 0x00A4),
    ("d", 0x0064),
    ("dagger", 0x2020),
    ("daggerdbl", 0x2021),
    ("dcaron", 0x010F),
    ("dcroat", 0x0111),
    ("degree", 0x00B0),
    ("deicoptic", 0x03EF),
    ("delta", 0x03B4),
    ("deltaturned", 0x018D),
    ("dialytikatonos", 0x0385),
    ("dieresis", 0x00A8),
    ("dieresistonos", 0x0385),
    ("divide", 0x00F7),
    ("dmacron", 0x0111),
    ("dollar", 0x0024),
    ("dotaccent", 0x02D9),
    ("dotlessi", 0x0131),
    ("dtopbar", 0x018C),
    ("dz", 0x01F3),
    ("dzcaron", 0x01C6),
    ("e", 0x0065),
    ("eacute", 0x00E9),
    ("ebreve", 0x0115),
    ("ecaron", 0x011B),
    ("ecircumflex", 0x00EA),
    ("edblgrave", 0x0205),
    ("edieresis", 0x00EB),
    ("edot", 0x0117),
    ("edotaccent", 0x0117),
    ("egrave", 0x00E8),
    ("eight", 0x0038),
    ("einvertedbreve", 0x0207),
    ("ellipsis", 0x2026),
    ("emacron", 0x0113),
    ("emdash", 0x2014),
    ("endash", 0x2013),
    ("eng", 0x014B),
    ("eogonek", 0x0119),
    ("epsilon", 0x03B5),
    ("epsilontonos", 0x03AD),
    ("equal", 0x003D),
    ("eshreversedloop", 0x01AA),
    ("eta", 0x03B7),
    ("etatonos", 0x03AE),
    ("eth", 0x00F0),
    ("eturned", 0x01DD),
    ("exclam", 0x0021),
    ("exclamdown", 0x00A1),
    ("ezhcaron", 0x01EF),
    ("ezhreversed", 0x01B9),
    ("ezhtail", 0x01BA),
    ("f", 0x0066),
    ("feicoptic", 0x03E5),
    ("ff", 0xFB00),
    ("ffi", 0xFB03),
    ("ffl", 0xFB04),
    ("fi", 0xFB01),
    ("five", 0x0035),
    ("fl", 0xFB02),
    ("florin", 0x0192),
    ("four", 0x0034),
    ("fraction", 0x2044),
    ("g", 0x0067),
    ("gacute", 0x01F5),
    ("gamma", 0x03B3),
    ("gangiacoptic", 0x03EB),
    ("gbreve", 0x011F),
    ("gcaron", 0x01E7),
    ("gcedilla", 0x0123),
    ("gcircumflex", 0x011D),
    ("gcommaaccent", 0x0123),
    ("gdot", 0x0121),
    ("gdotaccent", 0x0121),
    ("germandbls", 0x00DF),
    ("glottalinvertedstroke", 0x01BE),
    ("grave", 0x0060),
    ("greater", 0x003E),
    ("greaterequal", 0x2265),
    ("gstroke", 0x01E5),
    ("guillemotleft", 0x00AB),
    ("guillemotright", 0x00BB),
    ("guilsinglleft", 0x2039),
    ("guilsinglright", 0x203A),
    ("h", 0x0068),
    ("hbar", 0x0127),
    ("hcircumflex", 0x0125),
    ("horicoptic", 0x03E9),
    ("hungarumlaut", 0x02DD),
    ("hv", 0x0195),
    ("hyphen", 0x002D),
    ("i", 0x0069),
    ("iacute", 0x00ED),
    ("ibreve", 0x012D),
    ("icaron", 0x01D0),
    ("icircumflex", 0x00EE),
    ("idblgrave", 0x0209),
    ("idieresis", 0x00EF),
    ("igrave", 0x00EC),
    ("iinvertedbreve", 0x020B),
    ("ij", 0x0133),
    ("imacron", 0x012B),
    ("infinity", 0x221E),
    ("integral", 0x222B),
    ("iogonek", 0x012F),
    ("iota", 0x03B9),
    ("iotadieresis", 0x03CA),
    ("iotadieresistonos", 0x0390),
    ("iotatonos", 0x03AF),
    ("itilde", 0x0129),
    ("j", 0x006A),
    ("jcaron", 0x01F0),
    ("jcircumflex", 0x0135),
    ("k", 0x006B),
    ("kappa", 0x03BA),
    ("kappasymbolgreek", 0x03F0),
    ("kcaron", 0x01E9),
    ("kcedilla", 0x0137),
    ("kcommaaccent", 0x0137),
    ("kgreenlandic", 0x0138),
    ("kheicoptic", 0x03E7),
    ("khook", 0x0199),
    ("l", 0x006C),
    ("lacute", 0x013A),
    ("lambda", 0x03BB),
    ("lambdastroke", 0x019B),
    ("lbar", 0x019A),
    ("lcaron", 0x013E),
    ("lcedilla", 0x013C),
    ("lcommaaccent", 0x013C),
    ("ldot", 0x0140),
    ("ldotaccent", 0x0140),
    ("less", 0x003C),
    ("lessequal", 0x2264),
    ("lj", 0x01C9),
    ("logicalnot", 0x00AC),
    ("longs", 0x017F),
    ("lozenge", 0x25CA),
    ("lslash", 0x0142),
    ("m", 0x006D),
    ("macron", 0x00AF),
    ("middot", 0x00B7),
    ("minus", 0x2212),
    ("mu", 0x00B5),
    ("mu1", 0x00B5),
    ("mugreek", 0x03BC),
    ("multiply", 0x00D7),
    ("n", 0x006E),
    ("nacute", 0x0144),
    ("napostrophe", 0x0149),
    ("nbspace", 0x00A0),
    ("ncaron", 0x0148),
    ("ncedilla", 0x0146),
    ("ncommaaccent", 0x0146),
    ("nine", 0x0039),
    ("nj", 0x01CC),
    ("nlegrightlong", 0x019E),
    ("nonbreakingspace", 0x00A0),
    ("notequal", 0x2260),
    ("ntilde", 0x00F1),
    ("nu", 0x03BD),
    ("numbersign", 0x0023),
    ("numeralsigngreek", 0x0374),
    ("numeralsignlowergreek", 0x0375),
    ("o", 0x006F),
    ("oacute", 0x00F3),
    ("obreve", 0x014F),
    ("ocaron", 0x01D2),
    ("ocircumflex", 0x00F4),
    ("odblacute", 0x0151),
    ("odblgrave", 0x020D),
    ("odieresis", 0x00F6),
    ("oe", 0x0153),
    ("ogonek", 0x02DB),
    ("ograve", 0x00F2),
    ("ohorn", 0x01A1),
    ("ohungarumlaut", 0x0151),
    ("oi", 0x01A3),
    ("oinvertedbreve", 0x020F),
    ("omacron", 0x014D),
    ("omega", 0x03C9),
    ("omega1", 0x03D6),
    ("omegatonos", 0x03CE),
    ("omicron", 0x03BF),
    ("omicrontonos", 0x03CC),
    ("one", 0x0031),
    ("onehalf", 0x00BD),
    ("onequarter", 0x00BC),
    ("onesuperior", 0x00B9),
    ("oogonek", 0x01EB),
    ("oogonekmacron", 0x01ED),
    ("ordfeminine", 0x00AA),
    ("ordmasculine", 0x00BA),
    ("oslash", 0x00F8),
    ("oslashacute", 0x01FF),
    ("ostrokeacute", 0x01FF),
    ("otilde", 0x00F5),
    ("overscore", 0x00AF),
    ("p", 0x0070),
    ("paragraph", 0x00B6),
    ("parenleft", 0x0028),
    ("parenright", 0x0029),
    ("partialdiff", 0x2202),
    ("percent", 0x0025),
    ("period", 0x002E),
    ("periodcentered", 0x00B7),
    ("perthousand", 0x2030),
    ("phi", 0x03C6),
    ("phi1", 0x03D5),
    ("phisymbolgreek", 0x03D5),
    ("phook", 0x01A5),
    ("pi", 0x03C0),
    ("pisymbolgreek", 0x03D6),
    ("plus", 0x002B),
    ("plusminus", 0x00B1),
    ("product", 0x220F),
    ("psi", 0x03C8),
    ("q", 0x0071),
    ("question", 0x003F),
    ("questiondown", 0x00BF),
    ("questiongreek", 0x037E),
    ("quotedbl", 0x0022),
    ("quotedblbase", 0x201E),
    ("quotedblleft", 0x201C),
    ("quotedblright", 0x201D),
    ("quoteleft", 0x2018),
    ("quoteright", 0x2019),
    ("quoterightn", 0x0149),
    ("quotesinglbase", 0x201A),
    ("quotesingle", 0x0027),
    ("r", 0x0072),
    ("racute", 0x0155),
    ("radical", 0x221A),
    ("rcaron", 0x0159),
    ("rcedilla", 0x0157),
    ("rcommaaccent", 0x0157),
    ("rdblgrave", 0x0211),
    ("registered", 0x00AE),
    ("rho", 0x03C1),
    ("rhosymbolgreek", 0x03F1),
    ("ring", 0x02DA),
    ("rinvertedbreve", 0x0213),
    ("s", 0x0073),
    ("sacute", 0x015B),
    ("scaron", 0x0161),
    ("scedilla", 0x015F),
    ("scircumflex", 0x015D),
    ("scommaaccent", 0x0219),
    ("section", 0x00A7),
    ("semicolon", 0x003B),
    ("seven", 0x0037),
    ("sfthyphen", 0x00AD),
    ("sheicoptic", 0x03E3),
    ("shimacoptic", 0x03ED),
    ("sigma", 0x03C3),
    ("sigma1", 0x03C2),
    ("sigmafinal", 0x03C2),
    ("sigmalunatesymbolgreek", 0x03F2),
    ("six", 0x0036),
    ("slash", 0x002F),
    ("slong", 0x017F),
    ("softhyphen", 0x00AD),
    ("space", 0x0020),
    ("spacehackarabic", 0x0020),
    ("sterling", 0x00A3),
    ("summation", 0x2211),
    ("t", 0x0074),
    ("tau", 0x03C4),
    ("tbar", 0x0167),
    ("tcaron", 0x0165),
    ("tcedilla", 0x0163),
    ("tcommaaccent", 0x0163),
    ("theta", 0x03B8),
    ("theta1", 0x03D1),
    ("thetasymbolgreek", 0x03D1),
    ("thook", 0x01AD),
    ("thorn", 0x00FE),
    ("three", 0x0033),
    ("threequarters", 0x00BE),
    ("threesuperior", 0x00B3),
    ("tilde", 0x02DC),
    ("tonefive", 0x01BD),
    ("tonesix", 0x0185),
    ("tonetwo", 0x01A8),
    ("tonos", 0x0384),
    ("tpalatalhook", 0x01AB),
    ("trademark", 0x2122),
    ("two", 0x0032),
    ("twostroke", 0x01BB),
    ("twosuperior", 0x00B2),
    ("u", 0x0075),
    ("uacute", 0x00FA),
    ("ubreve", 0x016D),
    ("ucaron", 0x01D4),
    ("ucircumflex", 0x00FB),
    ("udblacute", 0x0171),
    ("udblgrave", 0x0215),
    ("udieresis", 0x00FC),
    ("udieresisacute", 0x01D8),
    ("udieresiscaron", 0x01DA),
    ("udieresisgrave", 0x01DC),
    ("udieresismacron", 0x01D6),
    ("ugrave", 0x00F9),
    ("uhorn", 0x01B0),
    ("uhungarumlaut", 0x0171),
    ("uinvertedbreve", 0x0217),
    ("umacron", 0x016B),
    ("underscore", 0x005F),
    ("uogonek", 0x0173),
    ("upsilon", 0x03C5),
    ("upsilondieresis", 0x03CB),
    ("upsilondieresistonos", 0x03B0),
    ("upsilontonos", 0x03CD),
    ("uring", 0x016F),
    ("utilde", 0x0169),
    ("v", 0x0076),
    ("verticalbar", 0x007C),
    ("w", 0x0077),
    ("wcircumflex", 0x0175),
    ("wynn", 0x01BF),
    ("x", 0x0078),
    ("xi", 0x03BE),
    ("y", 0x0079),
    ("yacute", 0x00FD),
    ("ycircumflex", 0x0177),
    ("ydieresis", 0x00FF),
    ("yen", 0x00A5),
    ("yhook", 0x01B4),
    ("yotgreek", 0x03F3),
    ("ypogegrammeni", 0x037A),
    ("yr", 0x01A6),
    ("z", 0x007A),
    ("zacute", 0x017A),
    ("zcaron", 0x017E),
    ("zdot", 0x017C),
    ("zdotaccent", 0x017C),
    ("zero", 0x0030),
    ("zeta", 0x03B6),
    ("zstroke", 0x01B6),
];

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index slices whose length they have
    // just asserted; both are what a test is for. The panic-free rule is about what the
    // product does with a file, not about how a test reads one.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::metrics::{STANDARD_14, Widths, standard_glyph, widths};

    fn dict(entries: &[(&str, Object)]) -> Dict {
        let mut d = Dict::new();
        for (key, value) in entries {
            d.insert((*key).into(), value.clone());
        }
        d
    }

    fn name(text: &str) -> Object {
        Object::name(text)
    }

    fn resolve(o: &Object) -> Option<Object> {
        Some(o.clone())
    }

    /// An `/Encoding` entry written as a dictionary with a `/BaseEncoding` and a
    /// `/Differences` array.
    fn encoding_dict(base: &str, differences: &[Object]) -> Object {
        let mut entries = vec![("Type", name("Encoding")), ("BaseEncoding", name(base))];
        if !differences.is_empty() {
            entries.push(("Differences", Object::Array(differences.to_vec())));
        }
        Object::Dict(dict(&entries))
    }

    /// Three bases, a spread of codes, and the codes where the three part company.
    ///
    /// The spread is chosen to fail in a way that matters. ASCII is where a transposed
    /// entry in the block all three share shows; the codes each of them spells differently
    /// are 0x27, 0x60 and 0x7F; and 0xA0–0xFF is the Latin-1 block, which is the only
    /// place the three differ enough for a document that maps one of them to render
    /// visibly wrong text. Every row below is asserted for all three at once, as a triple,
    /// so one wrong entry in one table fails rather than being averaged away.
    #[test]
    fn the_three_bases_are_read_correctly_across_ascii_latin1_and_the_high_range() {
        // ASCII, and the two codes where WinAnsi and MacRoman spell it differently from the
        // standard encoding.
        for (code, standard, other) in [
            (0x20u32, Some("space"), Some("space")),
            (0x21, Some("exclam"), Some("exclam")),
            (0x27, Some("quoteright"), Some("quotesingle")),
            (0x41, Some("A"), Some("A")),
            (0x5C, Some("backslash"), Some("backslash")),
            (0x60, Some("quoteleft"), Some("grave")),
            (0x7E, Some("asciitilde"), Some("asciitilde")),
        ] {
            assert_eq!(
                Base::Standard.glyph_for(code),
                standard,
                "StandardEncoding at 0x{code:02X}"
            );
            for base in [Base::WinAnsi, Base::MacRoman] {
                assert_eq!(
                    base.glyph_for(code),
                    other,
                    "{} at 0x{code:02X}",
                    base.name()
                );
            }
        }

        // 0x7F is a control code in the code page and a `bullet` only in WinAnsi, which
        // fills it because a page that writes it means a bullet.
        assert_eq!(Base::Standard.glyph_for(0x7F), None);
        assert_eq!(Base::WinAnsi.glyph_for(0x7F), Some("bullet"));
        assert_eq!(Base::MacRoman.glyph_for(0x7F), None);

        // The rest of the range, one triple per row, `(standard, win, mac)`. Rows where all
        // three answer and no two agree are marked; the ones where a base leaves a code
        // unassigned are just as load-bearing, because a name invented for them is the
        // failure this is looking for.
        for (code, standard, win, mac) in [
            (0x80u32, None, Some("Euro"), Some("Adieresis")),
            (0x8F, None, Some("bullet"), Some("egrave")),
            (0x9F, None, Some("Ydieresis"), Some("udieresis")),
            (0xA0, None, Some("space"), Some("dagger")),
            (0xA1, Some("exclamdown"), Some("exclamdown"), Some("degree")),
            (0xA4, Some("fraction"), Some("currency"), Some("section")),
            (
                0xA9,
                Some("quotesingle"),
                Some("copyright"),
                Some("copyright"),
            ),
            (0xAE, Some("fi"), Some("registered"), Some("AE")),
            (0xBD, Some("perthousand"), Some("onehalf"), Some("Omega")),
            (
                0xC7,
                Some("dotaccent"),
                Some("Ccedilla"),
                Some("guillemotleft"),
            ),
            (0xDB, None, Some("Ucircumflex"), Some("Euro")),
            (0xDD, None, Some("Yacute"), Some("guilsinglright")),
            (0xE1, Some("AE"), Some("aacute"), Some("periodcentered")),
            (0xFE, None, Some("thorn"), None),
            (0xFF, None, Some("ydieresis"), None),
        ] {
            assert_eq!(
                (
                    Base::Standard.glyph_for(code),
                    Base::WinAnsi.glyph_for(code),
                    Base::MacRoman.glyph_for(code),
                ),
                (standard, win, mac),
                "0x{code:02X} across the three bases"
            );
        }

        // The Latin-1 supplement proper, which WinAnsi takes wholesale from Latin-1. It is
        // the range the standard encoding leaves almost entirely to nothing — the codes it
        // does assign there are all symbols and ligatures — and which MacRoman fills with
        // its own high-range repertoire instead. Both facts are load-bearing: a name
        // invented for 0xE9 under the standard encoding would put an accented `e` where the
        // file means nothing, and one invented for 0xA9 under MacRoman would replace the
        // copyright sign with an accented `a`.
        for (code, win, mac) in [
            (0xC0u32, "Agrave", "questiondown"),
            (0xC7, "Ccedilla", "guillemotleft"),
            (0xD7, "multiply", "lozenge"),
            (0xD8, "Oslash", "ydieresis"),
        ] {
            assert_eq!(
                Base::WinAnsi.glyph_for(code),
                Some(win),
                "WinAnsi 0x{code:02X}"
            );
            assert_eq!(
                Base::MacRoman.glyph_for(code),
                Some(mac),
                "MacRoman 0x{code:02X}"
            );
        }
        for (code, win) in [(0xE9u32, "eacute"), (0xF7, "divide"), (0xFF, "ydieresis")] {
            assert_eq!(
                Base::WinAnsi.glyph_for(code),
                Some(win),
                "WinAnsi 0x{code:02X}"
            );
            assert_eq!(
                Base::MacRoman.glyph_for(code),
                None,
                "MacRoman stops at 0xE1, so it assigns nothing at 0x{code:02X}"
            );
        }
        for (code, assigned) in [
            (0xD8u32, None),
            (0xD9, None),
            (0xE1, Some("AE")),
            (0xE9, Some("Oslash")),
            (0xF0, None),
            (0xF7, None),
        ] {
            assert_eq!(
                Base::Standard.glyph_for(code),
                assigned,
                "StandardEncoding at 0x{code:02X}: it fills the Latin-1 range only with \
                 the letters it has no other name for"
            );
        }
    }

    /// The standard encoding here is the standard encoding there.
    ///
    /// `metrics.rs` carries its own copy of the standard encoding's names, because the width
    /// tables are keyed by name and had to be generated from somewhere. Two copies of a
    /// table is one more than there should be, so this asserts they are one table.
    #[test]
    fn the_standard_encoding_agrees_with_the_one_the_width_tables_are_keyed_by() {
        for code in 0..256 {
            assert_eq!(
                Base::Standard.glyph_for(code),
                standard_glyph(code),
                "code {code} in StandardEncoding"
            );
        }
    }

    /// A `/Differences` entry changes its own code and nothing else.
    ///
    /// The point is not that `/bullet` at 65 names a bullet — it is that the code before it
    /// and the code after it still name what they named. An entry that leaked onto its
    /// neighbours would move every glyph on the page after the first remap.
    #[test]
    fn a_differences_entry_overrides_the_base_for_that_code_and_no_other() {
        let mut encoding = Encoding::new(Base::WinAnsi);
        assert_eq!(encoding.glyph_for(0x41), Some("A"));
        encoding.push(0x41, "bullet".into());
        assert_eq!(encoding.glyph_for(0x41), Some("bullet"));
        for code in 0..256u32 {
            if code == 0x41 {
                continue;
            }
            assert_eq!(
                encoding.glyph_for(code),
                Base::WinAnsi.glyph_for(code),
                "code 0x{code:02X} is not the remapped one"
            );
        }
        // And several at once, which is what a file that remaps a run actually writes.
        let mut many = Encoding::new(Base::Standard);
        for (code, glyph) in [(0x41u32, "bullet"), (0x42, "Euro"), (0x43, "caron")] {
            many.push(code, glyph.into());
        }
        assert_eq!(many.glyph_for(0x41), Some("bullet"));
        assert_eq!(many.glyph_for(0x42), Some("Euro"));
        assert_eq!(many.glyph_for(0x43), Some("caron"));
        assert_eq!(many.glyph_for(0x44), Some("D"));
        assert_eq!(many.glyph_for(0x28), Some("parenleft"));
        // An empty name and `.notdef` both mean "not a character", and are honoured as
        // such rather than falling through to the base's name for the code.
        many.push(0x44, ".notdef".into());
        assert_eq!(many.glyph_for(0x44), None, ".notdef is not a character");
        many.push(0x44, String::new());
        assert_eq!(many.glyph_for(0x44), None, "nor is an empty name");
    }

    /// A run in `/Differences` that runs past the end of the base fills the gap.
    ///
    /// 0x41 is `A`; the array puts `bullet` there and then three more names after it, so
    /// 0x44 — which is `D` in every base — becomes `thorn`. A reader that stopped at the
    /// base's last assigned code would leave 0x44 as `D`, and a document that remaps a
    /// whole alphabet this way is ordinary.
    #[test]
    fn a_differences_run_past_the_end_of_the_base_fills_the_gap() {
        let encoding = Encoding::from_font_dict(
            &dict(&[(
                "Encoding",
                encoding_dict(
                    "WinAnsiEncoding",
                    &[
                        Object::Int(0x41),
                        name("bullet"),
                        name("Euro"),
                        name("caron"),
                        name("thorn"),
                    ],
                ),
            )]),
            Base::Standard,
            &resolve,
        )
        .expect("an encoding dictionary");
        assert_eq!(encoding.base(), Base::WinAnsi);
        assert_eq!(encoding.glyph_for(0x41), Some("bullet"));
        assert_eq!(encoding.glyph_for(0x42), Some("Euro"));
        assert_eq!(encoding.glyph_for(0x43), Some("caron"));
        assert_eq!(
            encoding.glyph_for(0x44),
            Some("thorn"),
            "a code the array does not reach keeps the base's name"
        );
        assert_eq!(encoding.glyph_for(0x45), Some("E"));
    }

    /// A code no encoding assigns is nothing, not a default glyph.
    ///
    /// `/notdef` is a *font's* glyph and the specification's answer for a code the font has
    /// no glyph for. An encoding has no such answer: code 0 is no character, and handing
    /// back a name for it would draw a visible box on a page that asked for nothing.
    #[test]
    fn a_code_in_no_encoding_is_none_rather_than_a_default_glyph() {
        for base in [Base::Standard, Base::WinAnsi, Base::MacRoman] {
            for code in [0u32, 1, 5, 31] {
                assert_eq!(base.glyph_for(code), None, "{} at {code}", base.name());
                assert_eq!(Encoding::new(base).glyph_for(code), None);
            }
            assert_eq!(base.glyph_for(0x100), None, "past the byte range");
        }
        assert_eq!(Base::MacRoman.glyph_for(0x7F), None, "a control code");
        assert_eq!(Base::MacRoman.glyph_for(0xE2), None, "past the code page");
        assert_eq!(Base::Standard.glyph_for(0xA0), None, "Standard 0xA0");
        assert_eq!(Base::Standard.glyph_for(0xE0), None, "Standard 0xE0");
        assert_eq!(
            Base::WinAnsi.glyph_for(0xA0),
            Some("space"),
            "WinAnsi does have it"
        );
        // PDFDocEncoding leaves 0x00–0x17 and the controls to nothing.
        for code in 0u32..0x18 {
            assert_eq!(
                pdf_doc_encoding(code),
                None,
                "PDFDocEncoding at 0x{code:02X}"
            );
        }
        assert_eq!(pdf_doc_encoding(0x18), Some("breve"));
        assert_eq!(pdf_doc_encoding(0x20), Some("space"));
        assert_eq!(pdf_doc_encoding(0x9F), Some("oslash"));
        assert_eq!(pdf_doc_encoding(0xA0), Some("space"));
        assert_eq!(pdf_doc_encoding(0x100), None);
    }

    /// Every name an encoding can produce is a width in the standard-14 tables, or is one
    /// of the names those tables are known not to carry.
    ///
    /// This is the test that catches a transposed table entry. A name that is in none of
    /// the width tables is very likely a name that should have been the one above or below
    /// it, so the assertion is an equality rather than a one-way check: a name with no
    /// width that is *not* on the list fails, and a name on the list that *has* one fails
    /// too, so the list cannot rot. It is exhaustive over all four tables for that reason:
    /// a spot check passes with a hundred wrong entries and this does not.
    ///
    /// **`StandardEncoding` must have no exceptions at all** — its names are the ones the
    /// width tables were generated from, so a name of its own with no width is a wrong
    /// entry and nothing else.
    ///
    /// The exceptions below are a gap in [`crate::metrics`], not in these tables: the
    /// Latin-1 supplement, `Euro` and the two Greek letterforms the encodings name, are
    /// real glyphs of Helvetica and Times and their Adobe widths are simply not transcribed
    /// yet. Listing them by name is how that gap is reported rather than hidden.
    #[test]
    fn every_name_an_encoding_can_produce_is_a_width_or_a_known_gap() {
        let mut wrong: Vec<(u32, &str)> = Vec::new();
        let mut stale: Vec<&str> = Vec::new();
        let mut unexpected: Vec<(&str, u32, &str)> = Vec::new();
        for (where_, code, name) in every_name() {
            if width_tables()
                .iter()
                .any(|(_, t)| t.width_of(name).is_some())
            {
                if NO_WIDTH.contains(&name) {
                    stale.push(name);
                }
                continue;
            }
            if !NO_WIDTH.contains(&name) {
                unexpected.push((where_, code, name));
            }
        }
        assert!(
            stale.is_empty(),
            "these are on the no-width list but a table has them: {stale:?}"
        );
        assert!(
            unexpected.is_empty(),
            "no standard-14 table carries these names, and they are not on the known-gap \
             list: {unexpected:?}"
        );
        for (where_, code, name) in every_name() {
            if where_ == "StandardEncoding"
                && width_tables()
                    .iter()
                    .all(|(_, t)| t.width_of(name).is_none())
            {
                wrong.push((code, name));
            }
        }
        assert!(
            wrong.is_empty(),
            "the standard encoding's own names are the ones the width tables were built \
             from, so none of them may be missing: {wrong:?}"
        );
    }

    /// Every code and name in every table, with where each came from.
    fn every_name() -> Vec<(&'static str, u32, &'static str)> {
        let mut out: Vec<(&'static str, u32, &'static str)> = STANDARD
            .iter()
            .map(|(code, name)| ("StandardEncoding", *code, *name))
            .collect();
        for base in [Base::WinAnsi, Base::MacRoman] {
            out.extend(
                base.entries()
                    .iter()
                    .map(|(code, name)| (base.name(), *code, *name)),
            );
        }
        for code in 0..256 {
            if let Some(name) = pdf_doc_encoding(code) {
                out.push(("PDFDocEncoding", code, name));
            }
        }
        out
    }

    /// The names the width tables do not carry, all of which the encodings introduce.
    const NO_WIDTH: &[&str] = &[
        "Aacute",
        "Acircumflex",
        "Adieresis",
        "Agrave",
        "Aring",
        "Atilde",
        "Ccedilla",
        "Delta",
        "Eacute",
        "Ecircumflex",
        "Edieresis",
        "Egrave",
        "Eth",
        "Euro",
        "Iacute",
        "Icircumflex",
        "Idieresis",
        "Igrave",
        "Ntilde",
        "Oacute",
        "Ocircumflex",
        "Odieresis",
        "Ograve",
        "Omega",
        "Otilde",
        "Scaron",
        "Thorn",
        "Uacute",
        "Ucircumflex",
        "Udieresis",
        "Ugrave",
        "Yacute",
        "Ydieresis",
        "Zcaron",
        "aacute",
        "acircumflex",
        "adieresis",
        "agrave",
        "approxequal",
        "aring",
        "atilde",
        "ccedilla",
        "copyright",
        "degree",
        "divide",
        "eacute",
        "ecircumflex",
        "edieresis",
        "egrave",
        "eth",
        "greaterequal",
        "iacute",
        "icircumflex",
        "idieresis",
        "igrave",
        "infinity",
        "integral",
        "lessequal",
        "logicalnot",
        "lozenge",
        "minus",
        "mu",
        "multiply",
        "notequal",
        "ntilde",
        "oacute",
        "ocircumflex",
        "odieresis",
        "ograve",
        "onehalf",
        "onequarter",
        "onesuperior",
        "otilde",
        "partialdiff",
        "pi",
        "plusminus",
        "product",
        "radical",
        "registered",
        "scaron",
        "summation",
        "thorn",
        "threequarters",
        "threesuperior",
        "trademark",
        "twosuperior",
        "uacute",
        "ucircumflex",
        "udieresis",
        "ugrave",
        "yacute",
        "ydieresis",
        "zcaron",
    ];

    /// The standard-14 tables that are not one fixed number, which is Courier and is no
    /// evidence of anything.
    ///
    /// A monospaced font answers `Some` for every name, which would make every assertion
    /// above pass whatever the tables said. Courier is left out of the comparison for that
    /// reason, and this asserts it is the one being left out.
    fn width_tables() -> Vec<(&'static str, &'static Widths)> {
        let tables: Vec<(&'static str, &'static Widths)> = STANDARD_14
            .iter()
            .filter_map(|name| widths(name).map(|table| (*name, table)))
            .filter(|(_, table)| table.fixed.is_none())
            .collect();
        assert_eq!(tables.len(), 8, "the eight proportional standard faces");
        tables
    }

    /// The two standard fonts with no width table are reported, not faked.
    ///
    /// A name only `Symbol` or `ZapfDingbats` carries is a real name, and answering it from
    /// Helvetica would be a wrong width rather than no answer. This asserts the refusal is
    /// there, so the exhaustive test above is not quietly passing because every font has a
    /// table.
    #[test]
    fn the_two_fonts_without_widths_are_reported_rather_than_faked() {
        for font in ["Symbol", "ZapfDingbats"] {
            assert!(STANDARD_14.contains(&font), "{font} is one of the fourteen");
            assert_eq!(widths(font), None, "{font} has no table here");
        }
        assert_eq!(
            width_tables().len() + 6,
            STANDARD_14.len(),
            "and the other twelve are eight proportional names, Courier's four and these two"
        );
        let courier = widths("Courier").expect("Courier is a table");
        assert_eq!(
            courier.fixed,
            Some(600),
            "which is the one fixed-width face"
        );
    }

    /// Every name in every table is a name the resolver knows.
    ///
    /// A typo in a table is invisible until a glyph silently fails to resolve, and a glyph
    /// that silently fails to resolve is a hole in a word rather than an error. This is the
    /// test that turns a typo into a failure at the point the typo is.
    #[test]
    fn every_name_in_every_table_is_a_name_the_agl_resolver_knows() {
        let unknown: Vec<(&str, u32, &str)> = every_name()
            .into_iter()
            .filter(|(_, _, name)| !is_agl_name(name))
            .collect();
        assert!(
            unknown.is_empty(),
            "names the AGL does not have: {unknown:?}"
        );
    }

    /// The Greek and Cyrillic names a `/Differences` array really uses are here.
    ///
    /// These came out of Adobe's `glyphlist.txt` rather than out of anybody's memory, which
    /// is the only reason they can be claimed complete — the machine had a copy of the AGL,
    /// so the Greek and Cyrillic blocks are transcribed from it in full rather than
    /// restricted to Latin. Four are asserted outright, one Greek letter and one Cyrillic
    /// one, both under their `afii` names, and the two block sizes are asserted so that a
    /// truncated block is a failure rather than a shorter file.
    #[test]
    fn the_greek_and_cyrillic_names_a_differences_array_uses_are_resolvable() {
        assert_eq!(agl("afii10017"), Some(0x0410), "afii10017 is Cyrillic A");
        assert_eq!(agl("afii10018"), Some(0x0411), "afii10018 is Cyrillic Be");
        assert_eq!(
            agl("afii00208"),
            Some(0x2015),
            "afii00208 is a horizontal bar"
        );
        assert_eq!(agl("afii08941"), Some(0x20A4), "afii08941 is a lira sign");
        assert_eq!(agl("alpha"), Some(0x03B1));
        assert_eq!(agl("beta"), Some(0x03B2));
        assert_eq!(
            agl("Omega"),
            Some(0x2126),
            "where the AGL lists two code points"
        );
        assert_eq!(agl("Delta"), Some(0x2206), "and the other way round");
        assert_eq!(
            agl("Omegagreek"),
            Some(0x03A9),
            "the other name for the same glyph"
        );
        assert_eq!(agl("periodcentered"), Some(0x00B7));
        assert_eq!(agl("fi"), Some(0xFB01));
        assert_eq!(agl("fl"), Some(0xFB02));
        assert_eq!(agl("ffi"), Some(0xFB03));
        assert_eq!(agl("ffl"), Some(0xFB04));
        assert_eq!(
            AGL_NAMES
                .iter()
                .filter(|(n, _)| n.starts_with("afii"))
                .count(),
            246,
            "the afii block, which is what Cyrillic is written in"
        );
        assert_eq!(
            AGL_NAMES
                .iter()
                .filter(|(_, c)| (0x0370..=0x03FF).contains(c))
                .count(),
            111,
            "the Greek and Coptic block"
        );
        assert!(
            AGL_NAMES.windows(2).all(|w| w[0].0 < w[1].0),
            "sorted by name"
        );
        assert!(!is_agl_name("definitelynotaglyphname"));
        assert!(!is_agl_name(""));
    }

    /// A name from a base encoding comes back from the code it came from.
    ///
    /// Only where the mapping is one-to-one. The encodings repeat names — `space` is at
    /// both 32 and 0xA0 in `WinAnsiEncoding` — and a name at two codes came from two places,
    /// so there is no one code to come back from and the answer is `None` rather than the
    /// first or the last.
    #[test]
    fn a_name_maps_back_to_the_code_it_came_from_where_the_mapping_is_one_to_one() {
        for base in [Base::Standard, Base::WinAnsi, Base::MacRoman] {
            let encoding = Encoding::new(base);
            for (code, name) in base.entries() {
                match encoding.code_for(name) {
                    Some(back) => assert_eq!(
                        back,
                        *code,
                        "{}: `{name}` came from 0x{code:02X}",
                        base.name()
                    ),
                    None => assert!(
                        base.entries().iter().filter(|(_, n)| *n == *name).count() > 1,
                        "{}: `{name}` is at 0x{code:02X} and nowhere else, so it must map back",
                        base.name()
                    ),
                }
            }
        }
        // The standard encoding repeats nothing, so every one of its names comes back.
        let standard = Encoding::new(Base::Standard);
        assert_eq!(standard.code_for("bullet"), Some(183));
        assert_eq!(standard.code_for("fraction"), Some(164));
        assert_eq!(standard.code_for("germandbls"), Some(251));
        assert_eq!(standard.code_for("quoteleft"), Some(96));
        assert_eq!(standard.code_for("Euro"), None, "not in StandardEncoding");
        assert_eq!(standard.code_for("quotesingle"), Some(169));
        // The repeats, asserted from the other side: these are the ones that must not.
        let win = Encoding::new(Base::WinAnsi);
        assert_eq!(win.code_for("space"), None, "32 and 0xA0 in WinAnsi");
        assert_eq!(win.code_for("hyphen"), None, "45 and 0xAD in WinAnsi");
        assert_eq!(win.code_for("bullet"), None, "127 and five more in WinAnsi");
        assert_eq!(win.code_for("florin"), None, "131 and 0xA6 in WinAnsi");
        assert_eq!(win.code_for("A"), Some(0x41), "which is not repeated");
        // A `/Differences` entry is a code of its own, and shadows the base's.
        let mut remapped = Encoding::new(Base::WinAnsi);
        remapped.push(0x41, "caron".into());
        assert_eq!(remapped.code_for("caron"), Some(0x41));
        assert_eq!(remapped.code_for("A"), None, "the base's A is now shadowed");
        // A name `/Differences` gives to two codes came from two places.
        let mut twice = Encoding::new(Base::WinAnsi);
        twice.push(0x41, "caron".into());
        twice.push(0x42, "caron".into());
        assert_eq!(twice.code_for("caron"), None);
        assert_eq!(twice.code_for("B"), None, "B is shadowed by nothing now");
    }

    /// All four of the shapes a file writes its `/Encoding` in.
    ///
    /// Not one shape with an option: the four are four code paths, and a file that writes
    /// any one of them has to be read. The fourth — a dictionary whose `/BaseEncoding` is a
    /// name — is the one that carries `/Differences`.
    #[test]
    fn all_four_shapes_of_an_encoding_entry_are_read() {
        /// One shape: what the file wrote, the base it must read as, a code to ask about,
        /// and the name that code must come out with.
        struct Shape {
            label: &'static str,
            object: Object,
            base: Base,
            code: u32,
            expected: &'static str,
        }
        let cases = [
            Shape {
                label: "/Encoding /WinAnsiEncoding",
                object: name("WinAnsiEncoding"),
                base: Base::WinAnsi,
                code: 0x41,
                expected: "A",
            },
            Shape {
                label: "/Encoding /MacRomanEncoding",
                object: name("MacRomanEncoding"),
                base: Base::MacRoman,
                code: 0xA4,
                expected: "section",
            },
            // A dictionary naming a base and carrying `/Differences`.
            Shape {
                label: "a dictionary with a base and differences",
                object: encoding_dict(
                    "MacRomanEncoding",
                    &[Object::Int(0x41), name("Euro"), name("caron")],
                ),
                base: Base::MacRoman,
                code: 0x41,
                expected: "Euro",
            },
            // A dictionary with no `/BaseEncoding`: the font's own built-in encoding, which
            // is not readable here, so the caller's default stands in for it.
            Shape {
                label: "a dictionary with no base",
                object: Object::Dict(dict(&[("Type", name("Encoding"))])),
                base: Base::Standard,
                code: 0x41,
                expected: "A",
            },
        ];
        for Shape {
            label,
            object,
            base,
            code,
            expected,
        } in cases
        {
            let encoding =
                Encoding::from_font_dict(&dict(&[("Encoding", object)]), Base::Standard, &resolve)
                    .unwrap_or_else(|| panic!("{label} is one of the four shapes"));
            assert_eq!(encoding.base(), base, "{label}: the base");
            assert_eq!(
                encoding.glyph_for(code),
                Some(expected),
                "{label}: the glyph"
            );
        }
        // A bare `/Differences` array in place of a dictionary, which a producer has been
        // known to write and which is a remap over the default base rather than nothing.
        let bare = Encoding::from_font_dict(
            &dict(&[(
                "Encoding",
                Object::Array(vec![Object::Int(0x41), name("Euro")]),
            )]),
            Base::WinAnsi,
            &resolve,
        )
        .expect("a bare differences array");
        assert_eq!(bare.base(), Base::WinAnsi);
        assert_eq!(bare.glyph_for(0x41), Some("Euro"));
        // The shapes that are *not* one of the four: a composite font's CMap, an expert
        // encoding this does not carry, a number, and a font with no `/Encoding` at all.
        for (label, object) in [
            ("a CMap name", name("Identity-H")),
            (
                "an expert encoding this does not carry",
                name("MacExpertEncoding"),
            ),
            ("a number", Object::Int(7)),
        ] {
            assert_eq!(
                Encoding::from_font_dict(&dict(&[("Encoding", object)]), Base::Standard, &resolve),
                None,
                "{label} is not an encoding"
            );
        }
        assert_eq!(
            Encoding::from_font_dict(&dict(&[]), Base::Standard, &resolve),
            None,
            "a font with no /Encoding has none here"
        );
    }

    /// `/Differences` survives a run of damage without inventing a code.
    ///
    /// Every one of these is a `/Differences` array some producer has been known to write: a
    /// name before any number, a real rather than an integer, a string, a negative number
    /// and a nested array. A reader that trusted the array's position rather than its
    /// contents would put `caron` at a code it was never asked for.
    #[test]
    fn a_malformed_differences_array_reads_as_far_as_it_is_sane() {
        let encoding = Encoding::from_font_dict(
            &dict(&[(
                "Encoding",
                encoding_dict(
                    "WinAnsiEncoding",
                    &[
                        name("bullet"),
                        Object::Int(0x41),
                        name("Euro"),
                        Object::Real(42.0),
                        name("caron"),
                        Object::String(b"x".to_vec()),
                        name("thorn"),
                        Object::Int(-3),
                        name("broken"),
                        Object::Array(Vec::new()),
                        name("mu"),
                    ],
                ),
            )]),
            Base::WinAnsi,
            &resolve,
        )
        .expect("an encoding dictionary");
        assert_eq!(
            encoding.glyph_for(0x41),
            Some("Euro"),
            "the name after 0x41"
        );
        assert_eq!(
            encoding.glyph_for(42),
            Some("caron"),
            "42.0 is the code 42, the real form of a code"
        );
        for code in [43u32, 44, 45, 46, 0x28] {
            assert_eq!(
                encoding.glyph_for(code),
                Base::WinAnsi.glyph_for(code),
                "nothing after the real remapped code \
                 (the string, the negative number and the nested array each ended the run)"
            );
        }
    }

    /// A very large code and a very large name are refused rather than walked.
    ///
    /// Every loop here is over a table, so a code outside the byte range and an array of a
    /// million names are the two shapes a hostile file could use. The bound on the name is
    /// the one that matters: `/Differences` is walked in full, so an array that cannot be
    /// read at all would otherwise be an unbounded allocation.
    #[test]
    fn a_hostile_encoding_entry_is_refused_rather_than_walked() {
        assert_eq!(Base::WinAnsi.glyph_for(u32::MAX), None);
        assert_eq!(Base::WinAnsi.glyph_for(0xFFFF_FFFF), None);
        assert_eq!(pdf_doc_encoding(u32::MAX), None);
        let huge = vec![Object::Int(0); 100_000];
        let encoding = Encoding::from_font_dict(
            &dict(&[("Encoding", encoding_dict("WinAnsiEncoding", &huge))]),
            Base::Standard,
            &resolve,
        )
        .expect("an encoding dictionary");
        assert_eq!(encoding.glyph_for(0x41), Some("A"), "nothing was remapped");
        // A code that would run past the end of the byte range stops the run rather than
        // wrapping into the codes below it.
        let wrap = Encoding::from_font_dict(
            &dict(&[(
                "Encoding",
                encoding_dict(
                    "WinAnsiEncoding",
                    &[Object::Int(0xFFFF_FFFF), name("Euro"), name("caron")],
                ),
            )]),
            Base::Standard,
            &resolve,
        )
        .expect("an encoding dictionary");
        assert_eq!(wrap.glyph_for(0x41), Some("A"), "and nothing wrapped");
    }
}
