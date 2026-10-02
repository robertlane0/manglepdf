//! How wide each glyph is.
//!
//! A viewer's job is to put the next character where the file said it goes, and the file
//! says that in one of two places. `/Widths` in the font dictionary is the first and covers
//! the codes the document actually uses; the fourteen standard fonts have no such
//! dictionary entry worth trusting for a document that names them without embedding them,
//! so their metrics are built in, which is what this file is.
//!
//! The tables are the Adobe Font Metrics widths, which is the specification's own source
//! for these fonts. They are keyed by glyph *name* rather than by character code, because
//! a name is what an encoding produces and a code is what a font is looked up by, and the
//! two disagree often enough that conflating them is how a page ends up with every `fi`
//! ligature the width of an `f`.
//!
//! Every number here was checked against an independent renderer rather than trusted: a
//! one-page PDF per font and per code, at 1000/1000 so the advances come out in text
//! space with no rounding, read back with `pdftotext -bbox` and with `mutool trace`. All
//! 1043 widths agree with both. One did not at first: `fraction` in the two Helvetica
//! faces was transcribed as 278, which is the Nimbus Sans clone's width rather than
//! Helvetica's own; the AFM says 167 and so does poppler, which is why the numbers here
//! are 167.
//!
//! ## What is deliberately not here
//!
//! Symbol and ZapfDingbats are not included: their widths belong to two fonts that no page
//! uses for prose, and a table copied approximately is worse than an absent one. A page
//! that names one is reported rather than laid out with invented numbers.

/// One standard font's widths, by glyph name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Widths {
    /// The name the font is known by in a page's `/BaseFont`.
    pub name: &'static str,
    /// The widths of the ASCII range, `space` (32) through `tilde` (126), in glyph order.
    /// Indexed from [`ASCII_START`].
    pub ascii: &'static [u16],
    /// Everything the standard encoding needs beyond ASCII, by glyph name.
    pub extra: &'static [(&'static str, u16)],
    /// Whether every glyph is the same width, which Courier is and which makes the table
    /// one number rather than two hundred.
    pub fixed: Option<u16>,
}

/// The first character code the ASCII range starts at.
pub const ASCII_START: u32 = 32;

/// The glyph names the standard encoding gives the codes from `space` to `tilde`.
///
/// Generated once so that the tables above are only numbers: a name written out beside
/// each one would be a hundred and forty-nine chances to mistype a width's owner.
#[must_use]
pub fn ascii_glyphs() -> &'static [&'static str] {
    ASCII_NAMES
}

/// The glyph the standard encoding gives a code, or `None` for a code it does not assign.
///
/// This is what turns a byte into something a table can be asked about, and it is the
/// standard encoding rather than a page's own because a page's own `/Encoding` is the font
/// layer's business and a page that supplies none gets this one.
#[must_use]
pub fn standard_glyph(code: u32) -> Option<&'static str> {
    if (ASCII_START..ASCII_START + ascii_glyphs().len() as u32).contains(&code) {
        let index = usize::try_from(code - ASCII_START).ok()?;
        return ascii_glyphs().get(index).copied();
    }
    STANDARD_ENCODING_ABOVE_ASCII
        .iter()
        .find(|(assigned, _)| *assigned == code)
        .map(|(_, name)| *name)
}

/// The fourteen standard font names, including the two with no table here.
///
/// A caller uses this to tell "this font is standard and we have it" from "this font is
/// standard and we do not", which are different findings from "this font is not standard".
pub const STANDARD_14: &[&str] = &[
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Symbol",
    "ZapfDingbats",
];

/// The built-in widths of a standard font, by the name a page spells it with.
///
/// A subset prefix (`ABCDEF+`) is noise around the name and is removed. The table is then
/// chosen by family *and* weight, because that is what the widths actually depend on:
///
/// * Helvetica: `-Oblique` is `-Roman` and `-BoldOblique` is `-Bold`, because an oblique
///   is the upright font slanted, which does not change how wide anything is.
/// * Times: `-Italic` is its own table and differs from `-Roman`, and so does
///   `-BoldItalic` from `-Bold`; a Times face really is a different set of glyphs.
/// * Courier: all four are the same table, because every glyph is 600.
///
/// The style is still recorded, in the returned table's `name`, so a caller that needs to
/// know which face it was handed can tell.
///
/// `None` means the file names a font this has no widths for, which is a real answer and
/// not a failure: a subset prefix with an unrecognised shape, a non-standard family, or a
/// suffix that changes the font rather than decorating it — `Helvetica-Narrow` is not
/// Helvetica, and answering it with Helvetica's widths would be a wrong answer rather than
/// no answer. A short name is not a font either: `Helv` is a guess at Helvetica that some
/// producer once wrote, and agreeing with that guess would turn it into our facts.
#[must_use]
pub fn widths(name: &str) -> Option<&'static Widths> {
    let (family, style) = split_name(name)?;
    let (bold, italic) = style_words(&style)?;
    Some(match family {
        "Helvetica" => {
            if bold {
                &HELVETICA_BOLD
            } else {
                &HELVETICA
            }
        }
        // A Times face really is a different set of glyphs, so unlike Helvetica each of
        // the four has its own widths.
        "Times" => match (bold, italic) {
            (false, false) => &TIMES_ROMAN,
            (false, true) => &TIMES_ITALIC,
            (true, false) => &TIMES_BOLD,
            (true, true) => &TIMES_BOLDITALIC,
        },
        "Courier" => &COURIER,
        _ => return None,
    })
}

/// Split a `/BaseFont` name into its family and its style words.
///
/// Three things are noise and are removed: a subset prefix, which is six characters and a
/// plus, and a trailing `MT` or `PS`, which is how a PostScript font spells the same face.
/// The family ends at the first hyphen or comma, so both the modern `Helvetica-BoldItalic`
/// and the older `Helvetica-Bold,Italic` come out as `Helvetica` and `BoldItalic`.
fn split_name(name: &str) -> Option<(&str, String)> {
    // A subset tag is exactly six characters; anything else before a plus is part of the
    // name, and a name with no plus has no tag at all.
    let name = match name.as_bytes().get(6) {
        Some(b'+') => name.get(7..)?,
        _ => name,
    };
    let end = name.find(['-', ',']).unwrap_or(name.len());
    let family = strip_face_suffix(name.get(..end)?);
    // Any further comma in the suffix separates two more style words rather than naming
    // another one, so `Helvetica-Bold,Italic` is `Helvetica` and `BoldItalic`.
    let style = name.get(end + 1..).unwrap_or("").replace(',', "");
    Some((family, strip_face_suffix(&style).to_string()))
}

/// Drop a `MT` or `PS` from the end of a name, however many are stacked.
fn strip_face_suffix(name: &str) -> &str {
    let mut name = name;
    while let Some(shorter) = name.strip_suffix("MT").or_else(|| name.strip_suffix("PS")) {
        name = shorter;
    }
    name
}

/// Read a style suffix as a pair of flags, or `None` when it is not one this file knows.
fn style_words(style: &str) -> Option<(bool, bool)> {
    let mut rest = style.to_ascii_lowercase();
    let mut bold = false;
    let mut italic = false;
    while !rest.is_empty() {
        let word = STYLE_WORDS.iter().find(|word| rest.starts_with(**word))?;
        let tail = rest.get(word.len()..)?;
        match *word {
            "bold" if !bold => bold = true,
            "italic" | "oblique" if !italic => italic = true,
            // `Roman` and `Regular` are the upright of a family, which every table for
            // that family already is. They are recognised so that `Times-Roman` is the
            // Times-Roman table rather than an unrecognised suffix.
            "roman" | "regular" if !bold && !italic => {}
            _ => return None,
        }
        rest = tail.to_string();
    }
    Some((bold, italic))
}

/// The style words the standard names are built from, longest-reading first.
///
/// A name's suffix is a run of these with nothing between them, which is why `Roman` is
/// listed: it is a word that says nothing rather than a word that changes the font.
const STYLE_WORDS: &[&str] = &["bold", "italic", "oblique", "roman", "regular"];

/// The width of one glyph in a standard font's table, or `None` when the font has no such
/// glyph.
///
/// A monospaced font answers for every name, which is what being monospaced means.
#[must_use]
pub fn by_name(table: &Widths, glyph: &str) -> Option<u16> {
    if let Some(width) = table.fixed {
        return Some(width);
    }
    // The names are in code order from `ASCII_START`, so a name's position in them is the
    // index into the ASCII row.
    if let Some(index) = ascii_glyphs().iter().position(|name| *name == glyph) {
        if let Some(width) = table.ascii.get(index) {
            return Some(*width);
        }
    }
    table
        .extra
        .iter()
        .find(|(name, _)| *name == glyph)
        .map(|(_, width)| *width)
}

impl Widths {
    /// The width of one glyph, or `None` when this font has none by that name.
    #[must_use]
    pub fn width_of(&self, glyph: &str) -> Option<u16> {
        by_name(self, glyph)
    }
}
// ---- generated tables ----
const HELVETICA: Widths = Widths {
    name: "Helvetica",
    ascii: &[
        278, 278, 355, 556, 556, 889, 667, 222, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 222, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ],
    extra: &[
        ("exclamdown", 333),
        ("cent", 556),
        ("sterling", 556),
        ("fraction", 167),
        ("yen", 556),
        ("florin", 556),
        ("section", 556),
        ("currency", 556),
        ("quotesingle", 191),
        ("quotedblleft", 333),
        ("guillemotleft", 556),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 500),
        ("fl", 500),
        ("endash", 556),
        ("dagger", 556),
        ("daggerdbl", 556),
        ("periodcentered", 278),
        ("paragraph", 537),
        ("bullet", 350),
        ("quotesinglbase", 222),
        ("quotedblbase", 333),
        ("quotedblright", 333),
        ("guillemotright", 556),
        ("ellipsis", 1000),
        ("perthousand", 1000),
        ("questiondown", 611),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 1000),
        ("AE", 1000),
        ("ordfeminine", 370),
        ("Lslash", 556),
        ("Oslash", 778),
        ("OE", 1000),
        ("ordmasculine", 365),
        ("ae", 889),
        ("dotlessi", 278),
        ("lslash", 222),
        ("oslash", 611),
        ("oe", 944),
        ("germandbls", 611),
    ],
    fixed: None,
};

const HELVETICA_BOLD: Widths = Widths {
    name: "Helvetica-Bold",
    ascii: &[
        278, 333, 474, 556, 556, 889, 722, 278, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722,
        722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 278, 556, 611, 556, 611, 556, 333, 611,
        611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556,
        500, 389, 280, 389, 584,
    ],
    extra: &[
        ("exclamdown", 333),
        ("cent", 556),
        ("sterling", 556),
        ("fraction", 167),
        ("yen", 556),
        ("florin", 556),
        ("section", 556),
        ("currency", 556),
        ("quotesingle", 238),
        ("quotedblleft", 500),
        ("guillemotleft", 556),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 611),
        ("fl", 611),
        ("endash", 556),
        ("dagger", 556),
        ("daggerdbl", 556),
        ("periodcentered", 278),
        ("paragraph", 556),
        ("bullet", 350),
        ("quotesinglbase", 278),
        ("quotedblbase", 500),
        ("quotedblright", 500),
        ("guillemotright", 556),
        ("ellipsis", 1000),
        ("perthousand", 1000),
        ("questiondown", 611),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 1000),
        ("AE", 1000),
        ("ordfeminine", 370),
        ("Lslash", 611),
        ("Oslash", 778),
        ("OE", 1000),
        ("ordmasculine", 365),
        ("ae", 889),
        ("dotlessi", 278),
        ("lslash", 278),
        ("oslash", 611),
        ("oe", 944),
        ("germandbls", 611),
    ],
    fixed: None,
};

const TIMES_ROMAN: Widths = Widths {
    name: "Times-Roman",
    ascii: &[
        250, 333, 408, 500, 500, 833, 778, 333, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667,
        722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722,
        722, 944, 722, 722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500,
        500, 278, 278, 500, 278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500,
        444, 480, 200, 480, 541,
    ],
    extra: &[
        ("exclamdown", 333),
        ("cent", 500),
        ("sterling", 500),
        ("fraction", 167),
        ("yen", 500),
        ("florin", 500),
        ("section", 500),
        ("currency", 500),
        ("quotesingle", 180),
        ("quotedblleft", 444),
        ("guillemotleft", 500),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 556),
        ("fl", 556),
        ("endash", 500),
        ("dagger", 500),
        ("daggerdbl", 500),
        ("periodcentered", 250),
        ("paragraph", 453),
        ("bullet", 350),
        ("quotesinglbase", 333),
        ("quotedblbase", 444),
        ("quotedblright", 444),
        ("guillemotright", 500),
        ("ellipsis", 1000),
        ("perthousand", 1000),
        ("questiondown", 444),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 1000),
        ("AE", 889),
        ("ordfeminine", 276),
        ("Lslash", 611),
        ("Oslash", 722),
        ("OE", 889),
        ("ordmasculine", 310),
        ("ae", 667),
        ("dotlessi", 278),
        ("lslash", 278),
        ("oslash", 500),
        ("oe", 722),
        ("germandbls", 500),
    ],
    fixed: None,
};

const TIMES_ITALIC: Widths = Widths {
    name: "Times-Italic",
    ascii: &[
        250, 333, 420, 500, 500, 833, 778, 333, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667,
        722, 611, 611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722,
        611, 833, 611, 556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500,
        500, 278, 278, 444, 278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444,
        389, 400, 275, 400, 541,
    ],
    extra: &[
        ("exclamdown", 389),
        ("cent", 500),
        ("sterling", 500),
        ("fraction", 167),
        ("yen", 500),
        ("florin", 500),
        ("section", 500),
        ("currency", 500),
        ("quotesingle", 214),
        ("quotedblleft", 556),
        ("guillemotleft", 500),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 500),
        ("fl", 500),
        ("endash", 500),
        ("dagger", 500),
        ("daggerdbl", 500),
        ("periodcentered", 250),
        ("paragraph", 523),
        ("bullet", 350),
        ("quotesinglbase", 333),
        ("quotedblbase", 556),
        ("quotedblright", 556),
        ("guillemotright", 500),
        ("ellipsis", 889),
        ("perthousand", 1000),
        ("questiondown", 500),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 889),
        ("AE", 889),
        ("ordfeminine", 276),
        ("Lslash", 556),
        ("Oslash", 722),
        ("OE", 944),
        ("ordmasculine", 310),
        ("ae", 667),
        ("dotlessi", 278),
        ("lslash", 278),
        ("oslash", 500),
        ("oe", 667),
        ("germandbls", 500),
    ],
    fixed: None,
};

const TIMES_BOLD: Widths = Widths {
    name: "Times-Bold",
    ascii: &[
        250, 333, 555, 500, 500, 1000, 833, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722,
        722, 667, 611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722,
        722, 1000, 722, 722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500,
        556, 278, 333, 556, 278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500,
        444, 394, 220, 394, 520,
    ],
    extra: &[
        ("exclamdown", 333),
        ("cent", 500),
        ("sterling", 500),
        ("fraction", 167),
        ("yen", 500),
        ("florin", 500),
        ("section", 500),
        ("currency", 500),
        ("quotesingle", 278),
        ("quotedblleft", 500),
        ("guillemotleft", 500),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 556),
        ("fl", 556),
        ("endash", 500),
        ("dagger", 500),
        ("daggerdbl", 500),
        ("periodcentered", 250),
        ("paragraph", 540),
        ("bullet", 350),
        ("quotesinglbase", 333),
        ("quotedblbase", 500),
        ("quotedblright", 500),
        ("guillemotright", 500),
        ("ellipsis", 1000),
        ("perthousand", 1000),
        ("questiondown", 500),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 1000),
        ("AE", 1000),
        ("ordfeminine", 300),
        ("Lslash", 667),
        ("Oslash", 778),
        ("OE", 1000),
        ("ordmasculine", 330),
        ("ae", 722),
        ("dotlessi", 278),
        ("lslash", 278),
        ("oslash", 500),
        ("oe", 722),
        ("germandbls", 556),
    ],
    fixed: None,
};

const TIMES_BOLDITALIC: Widths = Widths {
    name: "Times-BoldItalic",
    ascii: &[
        250, 389, 555, 500, 500, 833, 778, 333, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 832, 667, 667, 667,
        722, 667, 667, 722, 778, 389, 500, 667, 611, 889, 722, 722, 611, 722, 667, 556, 611, 722,
        667, 889, 667, 611, 611, 333, 278, 333, 570, 500, 333, 500, 500, 444, 500, 444, 333, 500,
        556, 278, 278, 500, 278, 778, 556, 500, 500, 500, 389, 389, 278, 556, 444, 667, 500, 444,
        389, 348, 220, 348, 570,
    ],
    extra: &[
        ("exclamdown", 389),
        ("cent", 500),
        ("sterling", 500),
        ("fraction", 167),
        ("yen", 500),
        ("florin", 500),
        ("section", 500),
        ("currency", 500),
        ("quotesingle", 278),
        ("quotedblleft", 500),
        ("guillemotleft", 500),
        ("guilsinglleft", 333),
        ("guilsinglright", 333),
        ("fi", 556),
        ("fl", 556),
        ("endash", 500),
        ("dagger", 500),
        ("daggerdbl", 500),
        ("periodcentered", 250),
        ("paragraph", 500),
        ("bullet", 350),
        ("quotesinglbase", 333),
        ("quotedblbase", 500),
        ("quotedblright", 500),
        ("guillemotright", 500),
        ("ellipsis", 1000),
        ("perthousand", 1000),
        ("questiondown", 500),
        ("grave", 333),
        ("acute", 333),
        ("circumflex", 333),
        ("tilde", 333),
        ("macron", 333),
        ("breve", 333),
        ("dotaccent", 333),
        ("dieresis", 333),
        ("ring", 333),
        ("cedilla", 333),
        ("hungarumlaut", 333),
        ("ogonek", 333),
        ("caron", 333),
        ("emdash", 1000),
        ("AE", 944),
        ("ordfeminine", 266),
        ("Lslash", 611),
        ("Oslash", 722),
        ("OE", 944),
        ("ordmasculine", 300),
        ("ae", 722),
        ("dotlessi", 278),
        ("lslash", 278),
        ("oslash", 500),
        ("oe", 722),
        ("germandbls", 500),
    ],
    fixed: None,
};

const COURIER: Widths = Widths {
    name: "Courier",
    ascii: &[],
    extra: &[],
    fixed: Some(600),
};

/// Every table above, so that a test can check all of them without repeating the list.
/// Nothing in the library needs it: [`widths`] is the lookup a caller makes.
#[cfg(test)]
const TABLES: &[&Widths] = &[
    &HELVETICA,
    &HELVETICA_BOLD,
    &TIMES_ROMAN,
    &TIMES_ITALIC,
    &TIMES_BOLD,
    &TIMES_BOLDITALIC,
    &COURIER,
];

/// The glyph names the standard encoding gives the codes 32 through 126.
const ASCII_NAMES: &[&str] = &[
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quoteright",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "quoteleft",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
];

/// The glyph names the standard encoding gives the codes above ASCII.
///
/// Only the codes the encoding actually assigns are here; the gaps are gaps in the
/// encoding rather than glyphs this file has forgotten.
const STANDARD_ENCODING_ABOVE_ASCII: &[(u32, &str)] = &[
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

/// One width as the file wrote it, in glyph space units.
///
/// A negative or absurd value is not a width, and a value too large to hold is not one
/// either; all of them are rejected. A caller that is building a positional run turns a
/// rejection into a zero rather than dropping the entry, because a missing entry would
/// shift every code after it by one place, which is a worse answer than a narrow glyph.
fn width_number(value: f64) -> Option<u16> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u16::MAX) {
        return None;
    }
    u16::try_from(value.round() as u32).ok()
}

/// The widths a font dictionary declares, for the codes a document actually uses.
///
/// `/Widths` is indexed from `/FirstChar` and covers a contiguous run of codes. A run that
/// stops short of a code the page uses means the font has no width for it, and the answer
/// is then the descriptor's `/MissingWidth` — which is a number the file supplies precisely
/// so that this case has an answer. Inventing one instead would put the rest of a line of
/// text out of position from the first character the dictionary did not cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// The first code the run covers.
    pub first: u32,
    /// The widths, one per code from `first` upwards.
    pub widths: Vec<u16>,
    /// What to use for a code outside the run, or `None` when the file said nothing.
    pub missing: Option<u16>,
}

impl Declared {
    /// Read `/FirstChar`, `/Widths` and the descriptor's `/MissingWidth` out of a font
    /// dictionary, or `None` when it declares no widths at all.
    ///
    /// `/Widths` without a `/FirstChar` starts at zero, which the specification says and
    /// which is why the first code is read rather than assumed to be there. `/Widths` and
    /// `/FontDescriptor` are usually indirect, so `resolve` is needed for either; a
    /// reference that does not resolve is treated as absent rather than as a width.
    #[must_use]
    pub fn from_font_dict(
        dict: &mangle_syntax::object::Dict,
        resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    ) -> Option<Self> {
        use mangle_syntax::object::Object;
        let direct = |key: &str| -> Option<Object> {
            let found = dict.get(key)?;
            Some(resolve(found).unwrap_or_else(|| found.clone()))
        };
        let array = direct("Widths")?.as_array()?.to_vec();
        // A rejected width becomes a zero in place: the run is indexed by code, so a hole
        // in it would put every later width on the wrong glyph.
        let widths: Vec<u16> = array
            .iter()
            .map(|value| value.as_f64().and_then(width_number).unwrap_or(0))
            .collect();
        let first = dict
            .get("FirstChar")
            .and_then(Object::as_i64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        let missing = dict
            .get("FontDescriptor")
            .map(|d| resolve(d).unwrap_or_else(|| d.clone()))
            .as_ref()
            .and_then(|d| d.as_dict())
            .and_then(|d| d.get("MissingWidth"))
            .and_then(Object::as_f64)
            .and_then(width_number);
        Some(Self {
            first,
            widths,
            missing,
        })
    }

    /// The width of one code, or `None` when neither the run nor `/MissingWidth` answers.
    #[must_use]
    pub fn width_of(&self, code: u32) -> Option<u16> {
        // A code below the run is as much outside it as one above it, so both fall to
        // `/MissingWidth`; the subtraction is what says which, and only when it succeeds
        // is there a row to read.
        if let Some(offset) = code.checked_sub(self.first) {
            if let Some(w) = self.widths.get(offset as usize) {
                return Some(*w);
            }
        }
        self.missing
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The names each table answers to besides its own.
    fn spellings(name: &str) -> Vec<String> {
        vec![
            name.to_string(),
            format!("ABCDEF+{name}"),
            format!("{name}MT"),
        ]
    }

    #[test]
    fn every_table_is_found_by_its_own_name_and_a_spelling_of_it() {
        for table in TABLES {
            for spelling in spellings(table.name) {
                let found = widths(&spelling);
                assert_eq!(
                    found.map(|t| t.name),
                    Some(table.name),
                    "{spelling} should find {}",
                    table.name
                );
            }
        }
    }

    #[test]
    fn a_style_suffixed_spelling_finds_the_table_with_those_widths() {
        // An oblique Helvetica is an upright Helvetica slanted, so the widths are the
        // upright's; a Times bold-italic really is different from Times bold.
        assert_eq!(
            widths("ABCDEF+Helvetica-BoldOblique").map(|t| t.name),
            Some("Helvetica-Bold")
        );
        assert_eq!(
            widths("Helvetica-Oblique").map(|t| t.name),
            Some("Helvetica")
        );
        assert_eq!(
            widths("Times-Bold,Italic").map(|t| t.name),
            Some("Times-BoldItalic")
        );
        assert_eq!(
            widths("Helvetica-Bold").map(|t| t.name),
            widths("Helvetica-BoldOblique").map(|t| t.name),
            "a bold oblique and a bold are the same widths"
        );
        assert_ne!(
            widths("Times-Bold").map(|t| t.name),
            widths("Times-BoldItalic").map(|t| t.name),
            "which is why Times keeps them apart"
        );
    }

    #[test]
    fn the_three_families_are_found() {
        assert!(widths("Helvetica").is_some());
        assert!(widths("Times-Roman").is_some());
        assert!(widths("Courier").is_some());
    }

    #[test]
    fn courier_is_six_hundred_for_every_glyph_it_is_asked_about() {
        let courier = widths("Courier").expect("Courier");
        assert_eq!(courier.fixed, Some(600));
        for glyph in [
            "space",
            "A",
            "a",
            "germandbls",
            "fi",
            "fl",
            "emdash",
            "ellipsis",
            "bullet",
            "quotedblleft",
            "Scaron",
            "uni0041",
        ] {
            assert_eq!(by_name(courier, glyph), Some(600), "{glyph} in Courier");
        }
        assert_eq!(courier.width_of("nothing-invented"), Some(600));
    }

    #[test]
    fn a_font_dictionary_is_read_into_a_run_of_widths() {
        use mangle_syntax::object::{Dict, Object};
        let mut descriptor = Dict::new();
        descriptor.set("MissingWidth", Object::Int(444));
        let mut font = Dict::new();
        font.set("FirstChar", Object::Int(65));
        font.set(
            "Widths",
            Object::Array(vec![Object::Int(722), Object::Int(667), Object::Int(667)]),
        );
        font.set("FontDescriptor", Object::Dict(descriptor));
        let d = Declared::from_font_dict(&font, &|o| Some(o.clone())).expect("a run");
        assert_eq!(d.first, 65);
        assert_eq!(d.widths, vec![722, 667, 667]);
        assert_eq!(d.missing, Some(444));
        assert_eq!(d.width_of(66), Some(667));
        assert_eq!(
            d.width_of(64),
            Some(444),
            "outside the run, from the descriptor"
        );
    }

    #[test]
    fn a_font_dictionary_with_no_widths_declares_nothing() {
        use mangle_syntax::object::{Dict, Object};
        let mut font = Dict::new();
        font.set("BaseFont", Object::name("Helvetica"));
        assert!(Declared::from_font_dict(&font, &|_| None).is_none());
    }

    #[test]
    fn an_indirect_descriptor_is_resolved_or_treated_as_absent() {
        use mangle_syntax::object::{Dict, Object, Ref};
        let mut descriptor = Dict::new();
        descriptor.set("MissingWidth", Object::Int(500));
        let mut font = Dict::new();
        font.set("FirstChar", Object::Int(32));
        font.set("Widths", Object::Array(vec![Object::Int(278)]));
        font.set("FontDescriptor", Object::Ref(Ref::new(7, 0)));

        let resolved = Declared::from_font_dict(&font, &|o| match o {
            Object::Ref(r) if r.num == 7 => Some(Object::Dict(descriptor.clone())),
            other => Some(other.clone()),
        })
        .expect("a run");
        assert_eq!(resolved.missing, Some(500));

        let unresolved = Declared::from_font_dict(&font, &|_| None).expect("a run");
        assert_eq!(
            unresolved.missing, None,
            "a reference that does not resolve is no answer, not a guess"
        );
        assert_eq!(unresolved.width_of(99), None);
        assert_eq!(
            unresolved.width_of(32),
            Some(278),
            "the run itself still reads"
        );
    }

    #[test]
    fn widths_without_a_first_char_start_at_zero() {
        use mangle_syntax::object::{Dict, Object};
        let mut font = Dict::new();
        font.set(
            "Widths",
            Object::Array(vec![Object::Int(10), Object::Int(20)]),
        );
        let d = Declared::from_font_dict(&font, &|_| None).expect("a run");
        assert_eq!(d.first, 0);
        assert_eq!(d.width_of(1), Some(20));
    }

    #[test]
    fn a_code_becomes_a_glyph_through_the_standard_encoding() {
        assert_eq!(standard_glyph(32), Some("space"));
        assert_eq!(standard_glyph(65), Some("A"));
        assert_eq!(standard_glyph(126), Some("asciitilde"));
        assert_eq!(standard_glyph(174), Some("fi"));
        assert_eq!(standard_glyph(251), Some("germandbls"));
        // The gaps are gaps in the encoding: 176 and 181 are unassigned.
        assert_eq!(standard_glyph(176), None);
        assert_eq!(standard_glyph(31), None);
        assert_eq!(standard_glyph(255), None);
    }

    #[test]
    fn every_name_the_encoding_produces_has_a_width_in_the_font_it_belongs_to() {
        // A name in the encoding but not in the table would advance a page by a guess, so
        // the two lists are checked against each other rather than trusted.
        for table in TABLES {
            for code in ASCII_START..=255 {
                let Some(name) = standard_glyph(code) else {
                    continue;
                };
                assert!(
                    by_name(table, name).is_some(),
                    "{} has no width for {name}, which code {code} produces",
                    table.name
                );
            }
        }
    }

    #[test]
    fn a_font_we_do_not_have_is_reported_rather_than_guessed() {
        assert!(widths("NotAFont").is_none());
        assert!(widths("Arial").is_none(), "not one of the fourteen");
        assert!(widths("Helvetica-Narrow").is_none(), "a different font");
        assert!(widths("Symbol").is_none(), "deliberately not included");
        assert!(widths("ZapfDingbats").is_none(), "nor this one");
    }

    #[test]
    fn a_short_name_is_not_a_font() {
        assert!(widths("Helv").is_none());
        assert!(widths("Times-R").is_none());
        assert!(widths("").is_none());
    }

    #[test]
    fn every_width_is_a_width_and_not_a_transcription_error() {
        for table in TABLES {
            for width in table.ascii.iter().copied().chain(table.fixed) {
                // Real Adobe widths are not all below 1000: Helvetica's `at` is 1015 and
                // `ellipsis` is 1000, so the bound is loose enough for the fonts and tight
                // enough that a dropped digit or a zero shows up.
                assert!(
                    (1..=2000).contains(&width),
                    "{} has a width of {width}",
                    table.name
                );
            }
            for (glyph, width) in table.extra {
                assert!(
                    (1..=2000).contains(width),
                    "{} gives {glyph} a width of {width}",
                    table.name
                );
            }
        }
    }

    #[test]
    fn an_ascii_glyph_is_looked_up_by_its_name_and_not_its_code() {
        let helvetica = widths("Helvetica").expect("Helvetica");
        assert_eq!(helvetica.width_of("space"), Some(278));
        assert_eq!(helvetica.width_of("A"), Some(667));
        assert_eq!(helvetica.width_of("a"), Some(556));
        // A name the standard encoding does not use is not in the ASCII range at all.
        assert_ne!(
            helvetica.width_of("fi"),
            Some(helvetica.width_of("f").unwrap())
        );
        assert_eq!(helvetica.width_of("fi"), Some(500));
        assert_eq!(helvetica.width_of("quotesinglbase"), Some(222));
        assert!(helvetica.width_of("notaglyph").is_none());
    }

    #[test]
    fn the_ascii_range_is_the_range_it_says_it_is() {
        let names = ascii_glyphs();
        assert_eq!(names.len(), 95, "32 through 126 inclusive");
        assert_eq!(ASCII_START, 32);
        assert_eq!(names.first().copied(), Some("space"));
        assert_eq!(names.last().copied(), Some("asciitilde"));
        for table in TABLES {
            if table.fixed.is_some() {
                continue;
            }
            assert_eq!(
                table.ascii.len(),
                names.len(),
                "{}'s ascii row lines up with the names",
                table.name
            );
        }
    }

    #[test]
    fn the_fourteen_names_are_the_ones_the_specification_lists() {
        assert_eq!(STANDARD_14.len(), 14);
        for name in [
            "Courier",
            "Helvetica",
            "Times-Roman",
            "Symbol",
            "ZapfDingbats",
        ] {
            assert!(STANDARD_14.contains(&name), "{name} is in the list");
        }
        for name in [
            "Courier-BoldOblique",
            "Helvetica-Oblique",
            "Times-BoldItalic",
        ] {
            assert!(STANDARD_14.contains(&name), "{name} is in the list");
        }
        // Symbol and ZapfDingbats are named but have no table, which is why the list and
        // the tables are two things.
        for name in ["Symbol", "ZapfDingbats"] {
            assert!(STANDARD_14.contains(&name) && widths(name).is_none());
        }
    }

    fn declared(first: u32, widths: &[u16], missing: Option<u16>) -> Declared {
        Declared {
            first,
            widths: widths.to_vec(),
            missing,
        }
    }

    #[test]
    fn a_code_inside_the_run_has_the_width_the_run_gives_it() {
        let d = declared(65, &[722, 667, 667], None);
        assert_eq!(d.width_of(65), Some(722));
        assert_eq!(d.width_of(67), Some(667));
    }

    #[test]
    fn the_first_and_last_code_of_the_run_are_both_covered() {
        let d = declared(32, &[278, 333, 250], None);
        assert_eq!(d.width_of(32), Some(278), "the first code");
        assert_eq!(d.width_of(34), Some(250), "the last code");
    }

    #[test]
    fn a_code_outside_the_run_falls_back_to_the_missing_width() {
        let d = declared(32, &[278, 333], Some(500));
        assert_eq!(d.width_of(31), Some(500), "one below the run");
        assert_eq!(d.width_of(34), Some(500), "one above the run");
    }

    #[test]
    fn a_code_outside_a_run_with_no_missing_width_has_no_answer() {
        let d = declared(65, &[722], None);
        assert_eq!(d.width_of(33), None);
        assert_eq!(d.width_of(64), None);
    }

    #[test]
    fn an_empty_run_answers_only_with_the_missing_width() {
        let d = declared(65, &[], Some(444));
        assert_eq!(d.width_of(65), Some(444));
        assert_eq!(d.width_of(64), Some(444));
        let silent = declared(65, &[], None);
        assert_eq!(silent.width_of(65), None);
    }

    #[test]
    fn a_run_that_does_not_start_at_zero_is_offset_by_its_first_char() {
        // `/FirstChar` is rarely 0, which is why the offset is computed rather than
        // assumed: taking the code as the index would read `A` out of the second entry.
        let d = declared(65, &[722, 667], None);
        assert_eq!(d.width_of(65), Some(722));
        assert_eq!(d.width_of(66), Some(667));
        assert_eq!(d.width_of(64), None);
    }
}
