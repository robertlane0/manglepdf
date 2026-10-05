//! Content streams: the operators, the state they act on, and the marks they produce.
//!
//! A content stream is a page's drawing instructions. Reading one means answering three
//! questions at once: what does this operation do to the state, what does it draw, and
//! **where in the file did it come from**. The third is what this crate is for. Every
//! token, every operator and every mark carries its byte range, which is what makes it
//! possible to select a shape on a page and change exactly the bytes that drew it.
//!
//! The layering is deliberate:
//!
//! | | |
//! |---|---|
//! | [`tokens`] | lexical, with spans: a stream becomes a list of typed items |
//! | [`ops`] | the operator table: what each operator consumes and does |
//! | [`state`] | the graphics state `q` and `Q` act on |
//! | [`interp`] | running a stream, producing marks with their spans |
//! | [`matrix`] | the 2-D affine transform, in PDF's own layout |
//!
//! What this crate deliberately does *not* do is decide what a glyph is. Deciding which
//! *bytes* of a string are glyphs needs the font's encoding, which belongs to
//! `mangle-font`, so here a byte is one glyph and that is stated rather than passed off as
//! a fact. How wide a glyph is is different: the font dictionary's own `/Widths` say, so
//! `Tf` reads them and every glyph is placed by its own declared width. A font that
//! declares none falls back to the conventional half-em average, which is also stated.

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

pub mod interp;
pub mod matrix;
pub mod ops;
pub mod state;
pub mod tokens;

use std::sync::Arc;

use mangle_font::metrics::{CidWidths, Declared, DeclaredWidths};

pub use interp::{
    BBox, FillRule, Mark, PageContent, Record, bbox_of, bounds_of, form_bbox, form_matrix, run,
    run_with, run_with_state,
};
pub use matrix::Matrix;
pub use state::{
    Clip, ClipBounds, Colour, ColourSpace, Dash, ExtGState, ExtGStates, GraphicsState, IccBased,
    LineCap, LineJoin, PathSegment, RenderMode, Rgba, StateStack, StrokeStyle, TextState,
};
pub use tokens::{ContentKind, ContentStream, ContentToken, Operation};

/// The resources a page's content stream refers to by name.
///
/// A content stream is full of names — `/F1`, `/Im0`, `/GS1`, `/Sh0` — and every one of
/// them is a lookup the interpreter has to do. Collecting the tables once means a stream
/// with a thousand `Do` operations does a thousand dictionary lookups against four
/// small maps rather than re-walking the resource dictionary each time.
#[derive(Debug, Clone, Default)]
pub struct Resources {
    pub fonts: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
    /// What each of those fonts declares about how wide its glyphs are.
    ///
    /// A font's `/Widths` and `/FontDescriptor` are usually indirect, and the interpreter
    /// has no resolver of its own, so they are followed here by the same resolver that
    /// built the table. A font that declares no widths has no entry here, which is a
    /// finding about the file rather than a width of zero.
    pub font_widths: std::collections::BTreeMap<String, DeclaredWidths>,
    /// The fonts whose character codes are two bytes wide.
    ///
    /// This is a fact about the *encoding*, not about the widths, so it is a separate table:
    /// a composite font that declares no `/W` at all still has two-byte codes, and a page
    /// that showed text in it would otherwise have its string split into single bytes.
    composite_fonts: std::collections::BTreeSet<String>,
    /// What each ICC-based colour space resource declares, by name.
    ///
    /// `/N` and `/Alternate` live in the profile stream's dictionary, which is normally an
    /// indirect object away from the `[/ICCBased …]` array that names it, so they are
    /// followed here through the same resolver that built the other tables: `cs`/`CS` run
    /// once per colour space a stream selects and hold no document, so this is the only
    /// place a reference can be followed. Only ICC-based spaces have an entry, which makes
    /// an absent one mean "this name is not an ICC-based space" rather than "nothing was
    /// read".
    icc: std::collections::BTreeMap<String, IccBased>,
    pub xobjects: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
    /// Every form XObject's own resource table, by name.
    ///
    /// A form carries its own `/Resources` and **everything it draws resolves against
    /// those**, so a font or an XObject named only inside a form is found here rather than
    /// in the page's tables. A form with no `/Resources` inherits, so it has *no entry*:
    /// an entry means "this form declares its own", and the absence of one is what
    /// [`Resources::form_resources`] reads as "use the table you were called with".
    ///
    /// The tables are built here rather than at execution because this is the only place
    /// that can follow a reference: `/Resources` is an indirect object in most files, and
    /// the interpreter deliberately holds no document to resolve it with.
    ///
    /// Shared rather than owned so that a page which draws the same form a thousand times
    /// pays for the table once, and so that a nested run can borrow it while the run that
    /// invoked it keeps its own.
    pub forms: std::collections::BTreeMap<String, Arc<Resources>>,
    pub ext_gstates: ExtGStates,
    pub shadings: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
    pub colour_spaces: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
    pub patterns: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
}

/// Two resource tables are the same when they name the same things.
impl PartialEq for Resources {
    fn eq(&self, other: &Self) -> bool {
        self.fonts == other.fonts
            && self.font_widths == other.font_widths
            && self.composite_fonts == other.composite_fonts
            && self.icc == other.icc
            && self.xobjects == other.xobjects
            && self.forms == other.forms
            && self.ext_gstates == other.ext_gstates
            && self.shadings == other.shadings
            && self.colour_spaces == other.colour_spaces
            && self.patterns == other.patterns
    }
}

impl Resources {
    /// Read a `/Resources` dictionary, with a way to resolve an indirect reference.
    #[must_use]
    pub fn from_dict(
        d: &mangle_syntax::object::Dict,
        resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    ) -> Self {
        Self::read_at(d, resolve, 0)
    }

    /// Read a table, `depth` form XObjects below the page.
    ///
    /// The depth is here so that a file whose forms name each other in a cycle stops
    /// building tables rather than recursing forever. It is the interpreter's own
    /// [`crate::interp::MAX_FORM_DEPTH`], not a second limit: a form nested past that
    /// cannot be executed, so its tables would never be consulted.
    fn read_at(
        d: &mangle_syntax::object::Dict,
        resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
        depth: usize,
    ) -> Self {
        let table = |key: &str| -> mangle_syntax::object::Object {
            d.get(key)
                .and_then(resolve)
                .or_else(|| d.get(key).cloned())
                .unwrap_or(mangle_syntax::object::Object::Dict(
                    mangle_syntax::object::Dict::new(),
                ))
        };
        let named = |obj: &mangle_syntax::object::Object| {
            let mut out = std::collections::BTreeMap::new();
            if let Some(inner) = obj.as_dict() {
                for (k, v) in inner.iter() {
                    let value = resolve(v).unwrap_or_else(|| v.clone());
                    out.insert(String::from_utf8_lossy(k.as_bytes()).into_owned(), value);
                }
            }
            out
        };
        // A `/gs` name resolves against a table whose values are usually references, so
        // the values are dereferenced here and the table is read from the result.
        let gs_values = named(&table("ExtGState"));
        let fonts = named(&table("Font"));
        let xobjects = named(&table("XObject"));
        // The colour spaces, and out of them the ICC-based ones. A colour space resource
        // is an array whose second element is the profile, so this is the one table whose
        // values have to be followed a second time: `named` dereferences the array, and
        // the profile inside it is dereferenced here. Anything that is not `[/ICCBased …]`
        // has no entry, which is what keeps the interpreter's lookup a single question.
        let colour_spaces = named(&table("ColorSpace"));
        let mut icc = std::collections::BTreeMap::new();
        for (name, value) in &colour_spaces {
            let Some(array) = value.as_array() else {
                continue;
            };
            if array
                .first()
                .and_then(mangle_syntax::object::Object::as_name)
                != Some(b"ICCBased")
            {
                continue;
            }
            let profile = array
                .get(1)
                .and_then(resolve)
                .or_else(|| array.get(1).cloned());
            icc.insert(name.clone(), IccBased::from_profile(profile.as_ref()));
        }
        // A form's own resources are read here, through the same resolver, rather than
        // when the form is executed: this is the only place a reference can be followed,
        // and a form's `/Resources` is an indirect object in most files.
        let forms = read_form_resources(&xobjects, resolve, depth);
        // The widths each font declares, read through the same resolver. A page usually
        // names two or three fonts, and reading them here is once per page rather than
        // once per `Tf`.
        //
        // A composite font declares no `/Widths` of its own — its widths are in the
        // descendant font's `/W` — so it is recognised and followed down, and the name is
        // recorded as a two-byte font whether or not the descendant declared anything.
        let mut composite_fonts = std::collections::BTreeSet::new();
        let mut font_widths = std::collections::BTreeMap::new();
        for (name, value) in &fonts {
            let Some(dict) = value.as_dict() else {
                continue;
            };
            if is_composite(dict) {
                composite_fonts.insert(name.clone());
                if let Some(widths) = descendant_widths(dict, resolve) {
                    font_widths.insert(name.clone(), DeclaredWidths::Composite(widths));
                }
                continue;
            }
            if let Some(declared) = Declared::from_font_dict(dict, resolve) {
                font_widths.insert(name.clone(), DeclaredWidths::Simple(declared));
                continue;
            }
            // No `/Widths`. For a document that names one of the standard fourteen without
            // embedding it this is not a gap in the file but the ordinary case — those fonts
            // have no `/Widths` by definition, and the widths are the ones the standard says
            // they are. Leaving the entry out makes the interpreter fall back to one average
            // advance for every glyph, which puts every character after the first in the
            // wrong place; reading the built-in table keeps the line breaks the producer
            // laid out against.
            let base = dict
                .get("BaseFont")
                .and_then(resolve)
                .or_else(|| dict.get("BaseFont").cloned())
                .and_then(|o| o.as_name().map(|n| String::from_utf8_lossy(n).into_owned()));
            let Some(base) = base else {
                continue;
            };
            let encoding = mangle_font::Encoding::from_font_dict(
                dict,
                mangle_font::EncodingBase::Standard,
                resolve,
            )
            .unwrap_or_else(|| mangle_font::Encoding::new(mangle_font::EncodingBase::Standard));
            if let Some(declared) = mangle_font::metrics::standard_run(&base, &encoding) {
                font_widths.insert(name.clone(), DeclaredWidths::Simple(declared));
            }
        }
        Self {
            font_widths,
            composite_fonts,
            fonts,
            xobjects,
            forms,
            shadings: named(&table("Shading")),
            colour_spaces,
            icc,
            patterns: named(&table("Pattern")),
            ext_gstates: {
                // A `/gs` name resolves against a table whose values are usually
                // references, so the values are dereferenced first and the table is read
                // from the result. A name that does not resolve is recorded rather than
                // dropped, because a page that used it was drawn with a state we do not
                // know.
                let mut entries = std::collections::BTreeMap::new();
                let mut missing = Vec::new();
                for (name, value) in &gs_values {
                    match value.as_dict() {
                        Some(inner) => {
                            entries.insert(name.clone(), ExtGState::from_dict(inner));
                        }
                        None => missing.push(name.clone()),
                    }
                }
                ExtGStates { entries, missing }
            },
        }
    }

    /// The `/ExtGState` a name refers to.
    #[must_use]
    pub fn ext_gstate(&self, name: &[u8]) -> Option<&ExtGState> {
        self.ext_gstates.get(name)
    }

    /// The resource table a form XObject of this name draws against.
    ///
    /// `None` when the form declares no `/Resources` of its own, which means it inherits
    /// the table it was named in rather than drawing with none — so a caller resolves
    /// against this when it is `Some` and against what it already has when it is `None`.
    ///
    /// Shared, so a page that draws the same form repeatedly reads one table, and so a
    /// nested run can hold this while the run that invoked it goes on.
    #[must_use]
    pub fn form_resources(&self, name: &str) -> Option<Arc<Resources>> {
        self.forms.get(name).cloned()
    }

    /// The widths the named font declares, or `None` when it declares none.
    ///
    /// This is what `Tf` reads: a font with no `/Widths` has no answer, and the caller
    /// falls back rather than inventing one.
    #[must_use]
    pub fn font_widths(&self, name: &str) -> Option<&DeclaredWidths> {
        self.font_widths.get(name)
    }

    /// Whether the named font's character codes are two bytes wide.
    ///
    /// This is what `Tf` records beside the name, and it is a separate question from the
    /// widths: a composite font that declares no `/W` still has two-byte codes, and a
    /// caller that inferred the code width from the presence of a widths array would split
    /// such a font's strings into single bytes and place every glyph after the first one
    /// wrongly. An unknown font is single-byte, which is the common case and the one that
    /// must not pay for the other.
    #[must_use]
    pub fn font_is_composite(&self, name: &str) -> bool {
        self.composite_fonts.contains(name)
    }

    /// What the named colour space's profile declares, when it selected one.
    ///
    /// `None` for a name that is not an `[/ICCBased …]` resource, and for an ICC-based
    /// resource whose profile could not be read — which reads as a profile that declares
    /// neither an `/N` nor an `/Alternate`, and is therefore reported rather than
    /// converted. A reader that resolves nothing sees every ICC space this way, which is
    /// the same answer it would give a profile with no alternate in it.
    #[must_use]
    pub fn icc_profile(&self, name: &str) -> Option<&IccBased> {
        self.icc.get(name)
    }

    /// Every name a content stream could refer to, for the Inspector.
    #[must_use]
    pub fn counts(&self) -> ResourceCounts {
        ResourceCounts {
            fonts: self.fonts.len(),
            xobjects: self.xobjects.len(),
            ext_gstates: self.ext_gstates.len(),
            shadings: self.shadings.len(),
            colour_spaces: self.colour_spaces.len(),
            patterns: self.patterns.len(),
        }
    }
}

/// Read each form XObject's own resource table, keyed by the name the page uses.
///
/// A form's `/Resources` is read through the same resolver as the page's, because it is
/// the same kind of object: in most files it is an indirect reference to a dictionary whose
/// entries are themselves indirect references, and following it once here is what makes a
/// font named only inside a form resolve.
///
/// A form that declares no `/Resources` gets no entry, which is how inheritance is
/// expressed — see [`Resources::forms`].
fn read_form_resources(
    xobjects: &std::collections::BTreeMap<String, mangle_syntax::object::Object>,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
    depth: usize,
) -> std::collections::BTreeMap<String, Arc<Resources>> {
    let mut forms = std::collections::BTreeMap::new();
    // At the depth the interpreter refuses to execute a form, its tables would never be
    // consulted, so they are not read: the bound here and the bound there are one bound.
    if depth >= interp::MAX_FORM_DEPTH {
        return forms;
    }
    for (name, object) in xobjects {
        let Some(dict) = xobject_dict(object) else {
            continue;
        };
        if !is_form(dict) {
            continue;
        }
        let Some(entry) = dict.get("Resources") else {
            continue;
        };
        let Some(resolved) = resolve(entry) else {
            continue;
        };
        let Some(inner) = resolved.as_dict() else {
            continue;
        };
        forms.insert(
            name.clone(),
            Arc::new(Resources::read_at(inner, resolve, depth + 1)),
        );
    }
    forms
}

/// The dictionary of an XObject, whichever of the two shapes it arrives in.
///
/// A form is a stream, because it has content; an image is a stream, because it has
/// samples. Both are read here so that `/Subtype` can be asked of either without the
/// caller matching on the object twice.
#[must_use]
pub fn xobject_dict(
    object: &mangle_syntax::object::Object,
) -> Option<&mangle_syntax::object::Dict> {
    match object {
        mangle_syntax::object::Object::Stream(s) => Some(&s.dict),
        mangle_syntax::object::Object::Dict(d) => Some(d),
        _ => None,
    }
}

/// Is this XObject dictionary a form?
///
/// The answer is in the dictionary's own `/Subtype` and nowhere else. Guessing from the
/// content — a stream that decodes as an image being an image, anything else being a form
/// — would report a form with a damaged image inside it as an image, and would execute a
/// broken image as a content stream. `/Subtype` is the file saying which it is.
///
/// A dictionary with no `/Subtype` is not a form. That is the legacy default of *image*,
/// which is what such files mean, and it is the reading that lets an image without the key
/// still be drawn.
#[must_use]
pub fn is_form(dict: &mangle_syntax::object::Dict) -> bool {
    dict.get("Subtype")
        .and_then(mangle_syntax::object::Object::as_name)
        == Some(b"Form")
}

/// Whether a font dictionary describes a composite (Type 0) font.
///
/// A composite font is the one whose character codes are two bytes. It says so itself:
/// `/Subtype` is `/Type0`, and the codes are then whatever its `/Encoding` maps — the
/// predefined `/Identity-H`, one of the other `-H`/`-V` CMap names, or a CMap stream the
/// file carries. Every one of those is a two-byte code, which is why the subtype alone is
/// enough to answer the question this function exists for.
///
/// A CMap stream is not parsed here, so a file whose CMap declared one-byte code space
/// ranges would be read as two-byte. That is the safe direction to be wrong in: the codes
/// are still two bytes in every font a page realistically uses, and a one-byte CMap is
/// rarer than a font this does not draw at all.
fn is_composite(dict: &mangle_syntax::object::Dict) -> bool {
    dict.get("Subtype")
        .and_then(mangle_syntax::object::Object::as_name)
        == Some(b"Type0")
}

/// The widths a composite font's descendant font declares, in its run-length `/W`.
///
/// `/DescendantFonts` is an array whose first entry is the CIDFont the codes are looked up
/// in. A file that names no descendant, or one that does not resolve, declares no widths —
/// which is a fact about the file rather than a width of zero, and the caller falls back.
fn descendant_widths(
    dict: &mangle_syntax::object::Dict,
    resolve: &dyn Fn(&mangle_syntax::object::Object) -> Option<mangle_syntax::object::Object>,
) -> Option<CidWidths> {
    let array = dict.get("DescendantFonts")?;
    let array = resolve(array).unwrap_or_else(|| array.clone());
    let first = array.as_array()?.first()?;
    let first = resolve(first).unwrap_or_else(|| first.clone());
    let descendant = first.as_dict()?;
    CidWidths::from_descendant_dict(descendant, resolve)
}

/// How many of each thing a page's resources hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceCounts {
    pub fonts: usize,
    pub xobjects: usize,
    pub ext_gstates: usize,
    pub shadings: usize,
    pub colour_spaces: usize,
    pub patterns: usize,
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use mangle_syntax::object::{Dict, Object};

    #[test]
    fn a_page_with_no_resources_has_none() {
        let r = Resources::from_dict(&Dict::new(), &|_| None);
        let c = r.counts();
        assert_eq!(c.fonts, 0);
        assert_eq!(c.xobjects, 0);
        assert!(r.ext_gstate(b"GS1").is_none());
    }

    #[test]
    fn resource_tables_are_read_by_name() {
        let mut font = Dict::new();
        font.set("F1", Object::Int(1));
        let mut xobject = Dict::new();
        xobject.set("Im0", Object::Int(2));
        let mut resources = Dict::new();
        resources.set("Font", Object::Dict(font));
        resources.set("XObject", Object::Dict(xobject));

        let r = Resources::from_dict(&resources, &|o| Some(o.clone()));
        let c = r.counts();
        assert_eq!(c.fonts, 1);
        assert_eq!(c.xobjects, 1);
        assert!(r.fonts.contains_key("F1"));
        assert!(r.xobjects.contains_key("Im0"));
    }

    /// A `/Pattern` entry lands in `patterns`, where `sh` looks for it.
    ///
    /// `sh` names a pattern rather than a shading, so a page that paints a gradient this
    /// way has named something the resource table would otherwise not keep.
    #[test]
    fn a_pattern_table_is_read_by_name() {
        let mut pattern = Dict::new();
        pattern.set("PatternType", Object::Int(2));
        let mut table = Dict::new();
        table.set("P0", Object::Ref(mangle_syntax::Ref::new(5, 0)));
        let mut resources = Dict::new();
        resources.set("Pattern", Object::Dict(table));

        let r = Resources::from_dict(&resources, &|o| match o {
            Object::Ref(r) if r.num == 5 => Some(Object::Dict(pattern.clone())),
            other => Some(other.clone()),
        });
        assert_eq!(r.counts().patterns, 1, "the page defines one pattern");
        let found = r.patterns.get("P0").expect("the pattern `/P0`");
        assert_eq!(
            found
                .as_dict()
                .and_then(|d| d.get("PatternType"))
                .and_then(Object::as_i64),
            Some(2),
            "and its value is the dictionary it stands for, not the reference"
        );
    }

    #[test]
    fn an_extgstate_table_is_read_even_when_its_values_are_references() {
        let mut inner = Dict::new();
        inner.set("LW", Object::Real(2.5));
        let mut table = Dict::new();
        table.set("GS1", Object::Ref(mangle_syntax::Ref::new(7, 0)));
        let mut resources = Dict::new();
        resources.set("ExtGState", Object::Dict(table));

        // A resolver that answers the reference with the dictionary it stands for.
        let r = Resources::from_dict(&resources, &|o| match o {
            Object::Ref(r) if r.num == 7 => Some(Object::Dict(inner.clone())),
            other => Some(other.clone()),
        });
        assert!(
            r.ext_gstates.missing.is_empty(),
            "{:?}",
            r.ext_gstates.missing
        );
        let gs = r.ext_gstate(b"GS1").expect("the extgstate");
        assert_eq!(gs.line_width, Some(2.5));
    }
}
