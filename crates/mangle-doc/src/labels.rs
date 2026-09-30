//! Page labels: the numbering scheme shown next to a page, which is not the same as
//! the page's position.
//!
//! `/PageLabels` is a list of ranges, each with a starting page index and a numbering
//! style. A page with no applicable range is a decimal number starting at 1 — which
//! is also what a reader must assume for a file with no `/PageLabels` at all.

use mangle_syntax::{Dict, Object};

use crate::Resolver;

/// One entry of a `/PageLabels` array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRange {
    /// The zero-based index of the first page this range covers.
    pub start: usize,
    /// `/S`: the numbering style.
    pub style: Style,
    /// `/P`: the prefix.
    pub prefix: Vec<u8>,
    /// `/St`: the first number, which defaults to 1.
    pub first: u32,
}

/// How a page number is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// No label at all: the range exists only to start a later one.
    None,
    #[default]
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetters,
    LowerLetters,
    /// `/D` with a non-integer `/P`: the prefix is the label and the number is hidden.
    PrefixOnly,
}

impl Style {
    /// The `/S` value for a numbering style. An unknown style is `/N` semantics, which
    /// is what a reader must assume rather than guess at.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"D" => Self::Decimal,
            b"R" => Self::UpperRoman,
            b"r" => Self::LowerRoman,
            b"A" => Self::UpperLetters,
            b"a" => Self::LowerLetters,
            _ => Self::None,
        }
    }
}

/// The complete labelling scheme.
#[derive(Debug, Clone, Default)]
pub struct PageLabel {
    ranges: Vec<LabelRange>,
}

impl PageLabel {
    /// Read `/PageLabels` from the catalogue.
    #[must_use]
    pub fn build(resolver: &dyn Resolver, catalog: &Dict) -> Self {
        let ranges = catalog
            .get("PageLabels")
            .and_then(|o| crate::follow(resolver, o.clone()))
            .and_then(|o| o.as_array().map(<[Object]>::to_vec))
            .map(|arr| arr.iter().filter_map(read_range).collect())
            .unwrap_or_default();
        Self { ranges }
    }

    /// An empty scheme, which numbers pages 1, 2, 3.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Build directly from ranges. Editing a scheme produces ranges rather than an
    /// array, and the editor should not have to serialise to re-read it.
    #[must_use]
    pub fn from_ranges(ranges: Vec<LabelRange>) -> Self {
        Self { ranges }
    }

    /// How many pages carry each kind of label, for the document properties dialog.
    #[must_use]
    pub fn ranges(&self) -> &[LabelRange] {
        &self.ranges
    }

    /// The label for a zero-based page index.
    #[must_use]
    pub fn label(&self, index: usize) -> Vec<u8> {
        let Some(range) = self.ranges.iter().rev().find(|r| r.start <= index) else {
            // No range covers it: the specification says to use decimal from 1.
            return format!("{}", index + 1).into_bytes();
        };
        if range.style == Style::None {
            return Vec::new();
        }
        let offset = (index - range.start) as u64;
        let n = u64::from(range.first) + offset;
        let number = match range.style {
            Style::Decimal | Style::PrefixOnly => n.to_string().into_bytes(),
            Style::UpperRoman => to_roman(n, true).into_bytes(),
            Style::LowerRoman => to_roman(n, false).into_bytes(),
            Style::UpperLetters => to_letters(n, true).into_bytes(),
            Style::LowerLetters => to_letters(n, false).into_bytes(),
            Style::None => Vec::new(),
        };
        let mut out = range.prefix.clone();
        out.extend_from_slice(&number);
        out
    }

    /// The label as text, for the UI.
    #[must_use]
    pub fn label_text(&self, index: usize) -> String {
        crate::outlines::decode_pdf_text(&self.label(index))
    }
}

fn read_range(obj: &Object) -> Option<LabelRange> {
    let d = obj.as_dict()?;
    let start = d.get("St").and_then(Object::as_i64).unwrap_or(0);
    let prefix = d
        .get("P")
        .and_then(Object::as_bytes)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    // `/P` that is not a string is a specification error. The reader's job is to show
    // the number without it, which is what treating the style as decimal-only does.
    let non_string_prefix = d.get("P").is_some() && d.get("P").and_then(Object::as_bytes).is_none();
    let style = match d.get("S").and_then(Object::as_name) {
        Some(name) => Style::from_name(name),
        None if non_string_prefix => Style::PrefixOnly,
        None => Style::Decimal,
    };
    Some(LabelRange {
        start: usize::try_from(start).unwrap_or(0),
        style,
        prefix,
        first: u32::try_from(
            d.get("P2")
                .and_then(Object::as_i64)
                .or_else(|| d.get("First").and_then(Object::as_i64))
                .unwrap_or(1),
        )
        .unwrap_or(1)
        .max(1),
    })
}

/// Roman numerals, capped at 3999 as the conventional maximum.
fn to_roman(mut n: u64, upper: bool) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const TABLE: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, sym) in TABLE {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    // The table is lower case, so the lower-case style is the one that needs no work.
    if upper { out.to_uppercase() } else { out }
}

/// Spreadsheet-style letters: 1 is `a`, 26 is `z`, 27 is `aa`.
fn to_letters(mut n: u64, upper: bool) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut out: Vec<char> = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push((b'a' + u8::try_from(rem).unwrap_or(0)) as char);
        n = (n - 1) / 26;
    }
    out.reverse();
    let s: String = out.into_iter().collect();
    if upper { s.to_uppercase() } else { s }
}
