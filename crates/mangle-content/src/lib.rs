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

use mangle_font::metrics::Declared;

pub use interp::{FillRule, Mark, PageContent, Record, bbox_of, bounds_of, run, run_with};
pub use matrix::Matrix;
pub use state::{
    ClipBounds, Colour, ColourSpace, Dash, ExtGState, ExtGStates, GraphicsState, LineCap, LineJoin,
    PathSegment, RenderMode, Rgba, StateStack, StrokeStyle, TextState,
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
    pub font_widths: std::collections::BTreeMap<String, Declared>,
    pub xobjects: std::collections::BTreeMap<String, mangle_syntax::object::Object>,
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
            && self.xobjects == other.xobjects
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
        // The widths each font declares, read through the same resolver. A page usually
        // names two or three fonts, and reading them here is once per page rather than
        // once per `Tf`.
        let font_widths = fonts
            .iter()
            .filter_map(|(name, value)| {
                let dict = value.as_dict()?;
                Some((name.clone(), Declared::from_font_dict(dict, resolve)?))
            })
            .collect();
        Self {
            font_widths,
            fonts,
            xobjects: named(&table("XObject")),
            shadings: named(&table("Shading")),
            colour_spaces: named(&table("ColorSpace")),
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

    /// The widths the named font declares, or `None` when it declares none.
    ///
    /// This is what `Tf` reads: a font with no `/Widths` has no answer, and the caller
    /// falls back rather than inventing one.
    #[must_use]
    pub fn font_widths(&self, name: &str) -> Option<&Declared> {
        self.font_widths.get(name)
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
