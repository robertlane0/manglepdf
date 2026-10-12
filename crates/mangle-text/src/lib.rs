//! Text extraction, reading order and search.
//!
//! The result is built from the page object model rather than from the raw content
//! stream, so a selection across two columns comes out in the order a reader would say
//! it is in, not in the order the operators happen to appear.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

// ---- decoding one run ----

/// One run of glyphs, as the interpreter recorded it.
///
/// The facts a run's bytes need to be read as text: where it was drawn (which resource table its
/// names resolve against), what it named its font, the string's own bytes, and where those bytes'
/// glyphs sat. The last one is not decoration — it is the only thing that says whether two runs
/// were one word or two.
#[derive(Debug, Clone)]
pub struct Run<'a> {
    /// The form XObject it was drawn in, if any.
    pub form: Option<&'a str>,
    /// The font as the stream named it — `/F3`, which is a resource name and not a family.
    pub font: Option<&'a str>,
    /// The string's own bytes, before any encoding is applied.
    pub text: &'a [u8],
    /// Each glyph's origin, in the same order as the character codes in `text`.
    ///
    /// The codes and the origins are two lists of the same length, and a decoder that read one
    /// without the other would be reading a page it could not lay out. Owned rather than borrowed
    /// because the origins come out of the interpreter's placement matrices, which are not laid
    /// out the way a caller happens to want them.
    pub origins: Vec<(f64, f64)>,
    /// The type size **as the reader sees it**, which is what a gap between runs is measured
    /// against.
    ///
    /// Not the number in the `Tf`. A file may set the font at one point and scale the whole run
    /// with a `cm` — which is ordinary in design exports — and then the `Tf` says `1` while the
    /// glyphs are twelve points tall. A gap measured against the stream's number is a gap measured
    /// against nothing: the threshold came out at a quarter of a point and every space in the page
    /// looked like a word break. The caller has the transformation in force, so the caller
    /// computes this: `size` scaled by how far the `cm` stretches.
    pub size: f64,
}

/// What one run's bytes turned out to be.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// The text. Empty when the run could not be read at all.
    pub text: String,
    /// How many character codes had no glyph in the font's encoding.
    ///
    /// Counted rather than dropped silently, because a page that loses half its letters this way
    /// is a page a user cannot read and has to be told about. `0` is the ordinary answer.
    pub unmapped: usize,
    /// Where the text starts and ends, in page space: the first and last glyph that was read.
    ///
    /// `None` when nothing was read, because a run with no text has no position either. These are
    /// what [`text_of`] joins runs with, and the reason a run carries its origins at all.
    pub extent: Option<((f64, f64), (f64, f64))>,
    /// The type size, kept so the caller can measure a gap in the units the page used.
    pub size: f64,
}

/// Read one run's bytes as text.
///
/// The path is the one the specification describes: the bytes are character *codes*, the font's
/// `/Encoding` says which glyph each code selects, and the glyph's name says which Unicode scalar
/// it is through the Adobe Glyph List. `fi` is one glyph and one code point (U+FB01), which is why
/// a ligature in a file is one character in the text and not two — and why searching for "office"
/// has to know about it.
///
/// `None` when the run has no readable font: a composite font's codes are CIDs and its `/Encoding`
/// is a CMap, so the byte is not a glyph selector and this has nothing to say. That is a real
/// finding about the file and a caller has to surface it rather than return an empty string and
/// call it a page with no text on it.
#[must_use]
pub fn decode(
    resources: &mangle_content::Resources,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    run: &Run<'_>,
) -> Option<Decoded> {
    // A form carries its own table, and a form with none inherits the page's — which is what an
    // absent entry means here, not "use nothing".
    let table = match run.form {
        Some(name) => resources
            .form_resources(name)
            .unwrap_or_else(|| std::sync::Arc::new(resources.clone())),
        None => std::sync::Arc::new(resources.clone()),
    };
    let name = run.font?;
    let font = table.fonts.get(name)?;
    let dict = font.as_dict()?;
    // A composite font is refused by the encoding reader, and refusing it here is correct: its
    // codes are two bytes and are CIDs, so there is no glyph per code to look up.
    if dict
        .get("Subtype")
        .and_then(|o| o.as_name())
        .is_some_and(|n| n == b"Type0")
    {
        return None;
    }
    let encoding =
        mangle_font::Encoding::from_font_dict(dict, mangle_font::EncodingBase::Standard, resolve)?;
    let mut out = Decoded {
        size: run.size,
        ..Decoded::default()
    };
    for (code, origin) in run
        .text
        .iter()
        .map(|b| u32::from(*b))
        .zip(run.origins.iter().copied())
    {
        let Some(glyph) = encoding.glyph_for(code) else {
            out.unmapped += 1;
            continue;
        };
        match mangle_font::agl(glyph).and_then(char::from_u32) {
            Some(c) => {
                let (ox, oy) = origin;
                let at = (ox, oy);
                out.extent = Some(match out.extent {
                    Some((first, _)) => (first, at),
                    None => (at, at),
                });
                out.text.push(c);
            }
            None => out.unmapped += 1,
        }
    }
    Some(out)
}

/// Join decoded runs into a page's text, in the order they are given.
///
/// **The separator comes from the geometry, not from the bytes.** A `Tj` names no space and a
/// file sets two words apart by placing the second further along the baseline, so joining runs
/// with nothing produces `NISTSpecialPublication800-88` and joining them all with a space
/// produces `NIS T Special` — both wrong. Two runs on different baselines are separated by a
/// newline; two on the same baseline are separated by a space when the gap between them is more
/// than a quarter of the type size, which is roughly a space's own width.
///
/// The threshold is a heuristic and it is stated as one: a page that sets its words a hair apart
/// will come out joined, and a page that kern its letters far apart will come out with spaces
/// inside words. Both are visible in the output, which is better than a separator that was
/// guessed from the bytes and could not be checked at all.
#[must_use]
pub fn text_of(runs: &[Decoded]) -> String {
    let mut out = String::new();
    let mut previous: Option<Placed> = None;
    for run in runs {
        let Some(extent) = run.extent else {
            continue;
        };
        let here = Placed::new(extent, run.size);
        if let Some(before) = previous {
            // A new baseline is a new line, and the larger of the two runs' sizes is what decides
            // how far apart two baselines have to be to count as different: a superscript sits
            // further from its line than its own size is tall.
            let tall = before.size.max(run.size);
            if (here.sy - before.ey).abs() > tall * 0.3 {
                out.push('\n');
            } else if here.sx - before.ex > tall * 0.25 {
                out.push(' ');
            }
        }
        out.push_str(&run.text);
        previous = Some(here);
    }
    out
}

/// Where one run's text sits, flattened out of [`Decoded::extent`].
///
/// A struct rather than a nested tuple because the four numbers are four different questions —
/// where the text starts, where it ends, and how big it is — and a type that read `((f64, f64),
/// (f64, f64), f64)` at the point of use answered none of them by name.
#[derive(Debug, Clone, Copy)]
struct Placed {
    /// Where the first glyph that was read sits.
    sx: f64,
    sy: f64,
    /// Where the last glyph that was read sits.
    ex: f64,
    ey: f64,
    /// The type size the reader sees.
    size: f64,
}

impl Placed {
    fn new(((sx, sy), (ex, ey)): ((f64, f64), (f64, f64)), size: f64) -> Self {
        Self {
            sx,
            sy,
            ex,
            ey,
            size,
        }
    }
}
