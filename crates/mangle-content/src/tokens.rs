//! The content-stream token stream: every operator and operand, with the bytes it
//! occupies.
//!
//! This is the whole reason the crate exists. A content stream has no delimiters, so a
//! reader must decide where a number ends and an operator begins, and once it has
//! decided it has thrown away the only thing that makes an edit surgical. Keeping the
//! span of every token means a selection can be turned back into a byte range, and one
//! token rewritten without touching any other.
//!
//! The lexical rules are the ones from the file syntax layer, so the lexer is shared
//! rather than reimplemented. Two things here are not in the file syntax:
//!
//! * **Inline images.** Between `ID` and `EI` the bytes are arbitrary binary that
//!   happens to look like operators. The only way through is to scan for the `EI`
//!   that is preceded and followed by whitespace, which is what the specification says
//!   marks the end.
//! * **Operators.** In a file a bare keyword is usually a mistake; in a content stream
//!   it is the point.

use std::ops::Range;

use mangle_syntax::lexer::{Lexer, Token};
use mangle_syntax::object::{Dict, Name, Object};

/// One item in a content stream, with the bytes it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentToken {
    /// What it is.
    pub kind: ContentKind,
    /// The exact bytes, in the original encoding.
    pub span: Range<usize>,
    /// The parsed value, for the kinds that have one.
    pub value: Object,
}

impl ContentToken {
    /// Is this an operator rather than an operand?
    #[must_use]
    pub fn is_operator(&self) -> bool {
        matches!(self.kind, ContentKind::Operator(_))
    }

    /// The operator's name, if this is an operator.
    #[must_use]
    pub fn operator(&self) -> Option<&[u8]> {
        match &self.kind {
            ContentKind::Operator(name) => Some(name),
            _ => None,
        }
    }

    /// The bytes this token occupies.
    #[must_use]
    pub fn bytes<'a>(&self, stream: &'a [u8]) -> &'a [u8] {
        stream.get(self.span.clone()).unwrap_or_default()
    }
}

/// What a content token is.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentKind {
    /// An operator, by name. `re`, `f`, `BT` and `Do` are operators.
    Operator(Vec<u8>),
    /// An inline image: its dictionary, and its data between `ID` and `EI`.
    InlineImage {
        /// The entries between `BI` and `ID`.
        dict: Dict,
        /// The bytes between `ID` and `EI`, unfiltered.
        data: Vec<u8>,
    },
    /// Anything that is an operand.
    Operand,
}

/// One operator with the operands it consumed.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// The operator.
    pub operator: ContentToken,
    /// The operands, in the order they appeared.
    pub operands: Vec<ContentToken>,
    /// Every byte this operation occupies, from the first operand to the operator.
    pub span: Range<usize>,
}

impl Operation {
    /// The operands as integers, ignoring any that are not.
    #[must_use]
    pub fn ints(&self) -> Vec<i64> {
        self.operands
            .iter()
            .filter_map(|t| t.value.as_i64())
            .collect()
    }

    /// The operands as reals, promoting an integer where the stream gave one.
    #[must_use]
    pub fn reals(&self) -> Vec<f64> {
        self.operands
            .iter()
            .filter_map(|t| t.value.as_f64())
            .collect()
    }

    /// The last `n` operands as reals, which is how an operator reads its arguments:
    /// the most recent operand is the last argument.
    #[must_use]
    pub fn last_reals(&self, n: usize) -> Vec<f64> {
        let all = self.reals();
        let skip = all.len().saturating_sub(n);
        all.into_iter().skip(skip).collect()
    }
}

/// Everything a content stream contains, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContentStream {
    tokens: Vec<ContentToken>,
    /// The bytes the spans refer to, kept so a caller need not hold them separately.
    source: Vec<u8>,
    /// What a collection bound cost, one note per affected collection.
    notes: Vec<String>,
}

impl ContentStream {
    /// Tokenise a content stream.
    ///
    /// This never fails. A byte the lexer cannot make sense of becomes an operand, which
    /// is the safe reading: it is skipped by the interpreter and the operations around
    /// it still work.
    #[must_use]
    pub fn parse(data: &[u8]) -> Self {
        let mut tokens = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut lex = Lexer::new(data);
        loop {
            // `skip_space` reports whether it passed a comment; whether it advanced is
            // the lexer's business, so its return value is not the loop's condition.
            lex.skip_space();
            if lex.at_end() {
                break;
            }
            let start = lex.position();
            let Ok(spanned) = lex.next_token() else {
                // Unreadable byte: consume it and move on rather than stopping, so a
                // damaged stream still yields the operations around the damage.
                if lex.position() <= start {
                    lex.seek(start.saturating_add(1));
                }
                if lex.at_end() {
                    break;
                }
                continue;
            };
            match &spanned.token {
                Token::Eof => break,
                Token::Keyword(k) if k.as_slice() == b"BI" => {
                    inline_image(data, &mut lex, start, &mut tokens);
                }
                Token::Keyword(name) => {
                    tokens.push(ContentToken {
                        kind: ContentKind::Operator(name.clone()),
                        span: spanned.start..spanned.end,
                        value: Object::Name(Name::from_bytes(name)),
                    });
                }
                // Arrays and dictionaries are gathered rather than emitted as brackets,
                // because the operators that take them need the contents: `TJ`'s kerning
                // and `d`'s dash pattern are both arrays, and an empty one silently
                // loses the spacing a page was authored with.
                Token::ArrayOpen => {
                    let (value, end) = collect(&mut lex, spanned.start, 1, b']', &mut notes);
                    tokens.push(ContentToken {
                        kind: ContentKind::Operand,
                        span: spanned.start..end,
                        value,
                    });
                }
                Token::DictOpen => {
                    let (value, end) = collect(&mut lex, spanned.start, 1, b'>', &mut notes);
                    tokens.push(ContentToken {
                        kind: ContentKind::Operand,
                        span: spanned.start..end,
                        value,
                    });
                }
                token => {
                    let value = object_of(token);
                    tokens.push(ContentToken {
                        kind: ContentKind::Operand,
                        span: spanned.start..spanned.end,
                        value,
                    });
                }
            }
        }
        Self {
            tokens,
            source: data.to_vec(),
            notes,
        }
    }

    /// The bytes this stream was parsed from.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.source
    }

    /// What a collection bound cost while this stream was read.
    ///
    /// A collection that hit [`MAX_COLLECTION_ITEMS`] or [`MAX_COLLECTION_DEPTH`] was
    /// shortened, and this says so by name and by count. The alternative — a shorter
    /// array and a dictionary with holes in it, with nothing to say which was meant — is
    /// a wrong answer that looks like a right one, and it is the one this project's own
    /// discipline is against everywhere else.
    #[must_use]
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Every token, in order.
    #[must_use]
    pub fn tokens(&self) -> &[ContentToken] {
        &self.tokens
    }

    /// Group the tokens into operations, each with the operands it consumed.
    ///
    /// A trailing run of operands with no operator is reported as one operation with an
    /// empty operator name, because it is in the file and the caller should see it.
    #[must_use]
    pub fn operations(&self) -> Vec<Operation> {
        let mut out = Vec::new();
        let mut pending: Vec<ContentToken> = Vec::new();
        for token in &self.tokens {
            if let ContentKind::InlineImage { .. } = &token.kind {
                let start = token.span.start;
                out.push(Operation {
                    operator: ContentToken {
                        kind: ContentKind::Operator(b"BI".to_vec()),
                        span: token.span.clone(),
                        value: Object::name("BI"),
                    },
                    span: start..token.span.end,
                    operands: vec![token.clone()],
                });
            } else if let ContentKind::Operator(name) = &token.kind {
                let start = pending
                    .first()
                    .map(|t| t.span.start)
                    .unwrap_or(token.span.start);
                out.push(Operation {
                    operator: token.clone(),
                    span: start..token.span.end,
                    operands: std::mem::take(&mut pending),
                });
                let _ = name;
            } else {
                pending.push(token.clone());
            }
        }
        if !pending.is_empty() {
            let start = pending.first().map(|t| t.span.start).unwrap_or(0);
            let end = pending.last().map(|t| t.span.end).unwrap_or(start);
            out.push(Operation {
                operator: ContentToken {
                    kind: ContentKind::Operator(Vec::new()),
                    span: end..end,
                    value: Object::Null,
                },
                span: start..end,
                operands: pending,
            });
        }
        out
    }

    /// The token covering a byte offset, which is how a click becomes a selection.
    #[must_use]
    pub fn token_at(&self, offset: usize) -> Option<&ContentToken> {
        self.tokens
            .iter()
            .find(|t| t.span.start <= offset && offset < t.span.end)
    }

    /// The operation covering a byte offset.
    #[must_use]
    pub fn operation_at(&self, offset: usize) -> Option<Operation> {
        self.operations()
            .into_iter()
            .find(|op| op.span.start <= offset && offset <= op.span.end)
    }

    /// The operators used, in the order first seen, with no repeats.
    #[must_use]
    pub fn operators(&self) -> Vec<Vec<u8>> {
        let mut seen: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
        let mut out: Vec<Vec<u8>> = Vec::new();
        for op in self.operations() {
            let name = op.operator.operator().unwrap_or_default().to_vec();
            if !name.is_empty() && seen.insert(name.clone()) {
                out.push(name);
            }
        }
        out
    }

    /// The resources an XObject name refers to, from the page's `/XObject` table.
    #[must_use]
    pub fn xobject_names(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for op in self.operations() {
            if op.operator.operator() == Some(&b"Do"[..])
                && let Some(name) = op.operands.last().map(|t| &t.value)
            {
                if let Object::Name(n) = name {
                    out.push(n.as_bytes().to_vec());
                }
            }
        }
        out
    }
}

fn object_of(token: &Token) -> Object {
    match token {
        Token::Null => Object::Null,
        Token::Bool(b) => Object::Bool(*b),
        Token::Int(i) => Object::Int(*i),
        Token::Real(r) => Object::Real(*r),
        Token::String(s) => Object::String(s.clone()),
        Token::HexString(s) => Object::String(s.clone()),
        Token::Name(n) => Object::Name(Name::from_bytes(n)),
        Token::ArrayOpen => Object::Array(Vec::new()),
        Token::ArrayClose => Object::Null,
        Token::DictOpen => Object::Dict(Dict::new()),
        Token::DictClose => Object::Null,
        Token::BraceOpen | Token::BraceClose => Object::Null,
        Token::Keyword(k) => Object::Name(Name::from_bytes(k)),
        Token::Eof => Object::Null,
    }
}

/// How deep a bracketed collection may nest, counting the outermost as one.
///
/// A file can open a bracket and never close it; without a bound the reader would gather
/// until the end of the stream and lose every operator after the bracket. This is the
/// bound for that, and it is small because nesting in a content stream is nearly always
/// incidental: the deepest thing a real stream contains is an optional-content membership
/// list inside a marked-content property list, which is two or three deep.
///
/// **Raising this constant is not the fix for a truncated array, and the depth test in this
/// module says so.** The two used to be one number, and the way out looked like making the
/// number bigger. At a depth bound of 200 000 the same hundred thousand unclosed `[` bytes
/// overflow the stack and abort the process — measured on a 2 MiB stack as well as an 8 MiB
/// one — and a crash is not a note. This bound is about nesting and
/// [`MAX_COLLECTION_ITEMS`] is about width; neither substitutes for the other.
pub const MAX_COLLECTION_DEPTH: usize = 32;

/// How many items one array or dictionary may hold.
///
/// **This is not [`MAX_COLLECTION_DEPTH`], and the two used to be one number, which
/// truncated every array in every text-heavy file in the corpus.** The commonest array in
/// a content stream is a `TJ`, and a `TJ` written by a typesetter is kerned per character:
/// one string per character and one number between each pair. So a full line of 12-point
/// text in a 600-point measure — a mean advance of about half an em, so a hundred
/// characters — is `2 × 100 − 1 = 199` items, and a line twice as wide is 399. The
/// measure the format allows is 14 400 points: at 12-point type that is 2 400 characters
/// and 4 799 items, at 8-point type 3 600 characters and 7 199 items. Both fit, with room
/// over. What this bound refuses is one show operation carrying a line of text thinner
/// than about 7 points stretched the full width of a page, which is not a document.
///
/// **What a hostile file pays.** One array at the bound is 8 192 `Object`s, and one
/// `Object` is 104 bytes on a 64-bit target, so 852 KB held. Filling it costs at least
/// two bytes per item in the file (`0 `), so 16 KB of hostile stream buys that 852 KB —
/// an amplification of 52×, and that ratio is fixed by the size of one object and the
/// two bytes an item needs, **not** by this constant: a file that wants more objects
/// opens more arrays, and every collection on the page is bounded separately. A page of
/// two thousand lines therefore holds two thousand arrays of a few hundred items rather
/// than one array that grows until the process is out of memory, and the excess is
/// reported rather than dropped in silence.
///
/// A dictionary entry is two items, a key and a value, so this is 4 096 entries; the
/// largest legitimate content-stream dictionary is a marked-content property list or a
/// membership list, both of tens of entries.
pub const MAX_COLLECTION_ITEMS: usize = 8192;

/// The empty collection a bracket of this kind opens.
fn empty_of(closer: u8) -> Object {
    if closer == b']' {
        Object::Array(Vec::new())
    } else {
        Object::Dict(Dict::new())
    }
}

/// How wide the bracket this collection opened with is, so the contents can be found.
///
/// An array opens with one `[` and a dictionary with two `<`, which is why the closer
/// decides it: `collect` is only ever entered at an opening bracket.
fn bracket_width(closer: u8) -> usize {
    if closer == b']' { 1 } else { 2 }
}

/// Gather the items of a bracketed collection, starting **at** its opening bracket.
///
/// `at` is the offset of that bracket, which is what a note about this collection has to
/// name, and the contents begin `bracket_width(closer)` bytes later. `depth` is how many
/// collections deep this one is, the outermost being one. `closer` is `]` or `>`. A
/// dictionary's `>>` arrives as one token: one is consumed here and the other is left for
/// the caller's next token to step over as a stray, which is harmless and better than
/// tracking half a bracket.
///
/// **Both bounds live here, and neither is the other.** Depth is what stops a file that
/// opens brackets and never closes them; the item count is what stops a file that fills
/// one. Reading to the depth bound and then **skipping** the rest of the collection in a
/// single pass is deliberate: skipping at each level in turn would re-walk the remainder
/// once per level, so a megabyte of unclosed brackets would cost thirty-two passes over it
/// instead of one.
fn collect(
    lex: &mut Lexer<'_>,
    at: usize,
    depth: usize,
    closer: u8,
    notes: &mut Vec<String>,
) -> (Object, usize) {
    let mut items: Vec<Object> = Vec::new();
    let mut dropped = 0usize;
    let mut end = at + bracket_width(closer);
    loop {
        lex.skip_space();
        if lex.at_end() {
            break;
        }
        let Ok(token) = lex.next_token() else {
            break;
        };
        end = token.end;
        match &token.token {
            Token::Eof => break,
            // The closing bracket. For a dictionary the file writes `>>`, so this returns
            // with one `>` still unread, which the caller's next token will see and skip
            // as a stray — harmless, and better than tracking half a bracket.
            Token::ArrayClose => {
                if closer == b']' {
                    break;
                }
            }
            Token::DictClose => {
                if closer == b'>' {
                    break;
                }
            }
            Token::ArrayOpen | Token::DictOpen => {
                if depth >= MAX_COLLECTION_DEPTH {
                    // Past the bound: walk the rest of this collection without reading it,
                    // and say so. What was already gathered is kept, because it is this
                    // file's and cost nothing.
                    end = skip_collection(lex, token.end);
                    notes.push(format!(
                        "the collection at byte {at} nests deeper than the {MAX_COLLECTION_DEPTH} \
                         levels allowed, so it and everything inside it were left unread"
                    ));
                    break;
                }
                let inner = if matches!(token.token, Token::ArrayOpen) {
                    b']'
                } else {
                    b'>'
                };
                let (value, inner_end) = collect(lex, token.start, depth + 1, inner, notes);
                end = inner_end;
                if items.len() < MAX_COLLECTION_ITEMS {
                    items.push(value);
                } else {
                    dropped += 1;
                }
            }
            other => {
                if items.len() < MAX_COLLECTION_ITEMS {
                    items.push(object_of(other));
                } else {
                    dropped += 1;
                }
            }
        }
    }
    if dropped > 0 {
        notes.push(format!(
            "the {} at byte {} holds more than the {MAX_COLLECTION_ITEMS} items allowed, so it \
             was cut short there and the rest of it was not read",
            if closer == b']' {
                "array"
            } else {
                "dictionary"
            },
            at
        ));
    }
    (finish(empty_of(closer), items), end)
}

/// Walk past a collection's contents without reading them, returning where it ended.
///
/// Used once the depth bound has been reached. The file still has to be walked off to
/// the matching bracket, or every operator after it would be read as part of a
/// collection nobody is reading.
fn skip_collection(lex: &mut Lexer<'_>, start: usize) -> usize {
    let mut depth = 1usize;
    let mut end = start;
    while depth > 0 {
        lex.skip_space();
        if lex.at_end() {
            return end;
        }
        let Ok(token) = lex.next_token() else {
            return end;
        };
        end = token.end;
        match &token.token {
            Token::Eof => return end,
            Token::ArrayOpen | Token::DictOpen => depth += 1,
            Token::ArrayClose | Token::DictClose => depth -= 1,
            _ => {}
        }
    }
    end
}

/// Put the gathered items into the collection they belong to.
fn finish(empty: Object, items: Vec<Object>) -> Object {
    match empty {
        Object::Array(_) => Object::Array(items),
        Object::Dict(_) => {
            // A dictionary is alternating keys and values, and a trailing key with no
            // value is damage the reader steps over rather than propagates.
            let mut dict = Dict::new();
            let mut iter = items.into_iter();
            while let (Some(key), Some(value)) = (iter.next(), iter.next()) {
                if let Object::Name(k) = key {
                    dict.insert(k, value);
                } else {
                    // A key that is not a name means the pairing is off by one from here
                    // on, so stop: guessing would produce a dictionary with the wrong keys.
                    break;
                }
            }
            Object::Dict(dict)
        }
        other => other,
    }
}

/// Read an inline image: `BI`, a flat dictionary, `ID`, one whitespace byte, the data,
/// then the `EI` that ends it.
fn inline_image(
    data: &[u8],
    lex: &mut Lexer<'_>,
    start: usize,
    out: &mut Vec<ContentToken>,
) -> usize {
    let mut dict = Dict::new();

    // The dictionary is a flat run of key/value pairs with no brackets.
    loop {
        lex.skip_space();
        if lex.at_end() {
            return data.len();
        }
        let Ok(spanned) = lex.next_token() else {
            return data.len();
        };
        match &spanned.token {
            Token::Eof => return data.len(),
            Token::Keyword(k) if k.as_slice() == b"ID" => {
                // Exactly one whitespace byte separates `ID` from the data.
                let after = spanned.end;
                let body_start = match data.get(after) {
                    Some(b) if b.is_ascii_whitespace() => after + 1,
                    Some(_) => after,
                    None => return data.len(),
                };
                let Some(body_end) = find_ei(data, body_start) else {
                    // No `EI`: the image runs to the end of the stream, which is what
                    // the file says and the only honest reading.
                    let body = data.get(body_start..).unwrap_or_default().to_vec();
                    push_image(out, start, dict, body, data.len());
                    return data.len();
                };
                let body = data.get(body_start..body_end).unwrap_or_default().to_vec();
                push_image(out, start, dict, body, body_end + 2);
                lex.seek(body_end + 2);
                return body_end + 2;
            }
            Token::Name(key) => {
                // The value follows; a missing one is damage we step over.
                lex.skip_space();
                let Ok(value) = lex.next_token() else {
                    return data.len();
                };
                dict.insert(Name::from_bytes(key), object_of(&value.token));
            }
            other => {
                // Not a key: this is not an inline image after all, so `BI` is an
                // ordinary operator and the token we just read is an operand. Dropping
                // it would silently lose a number.
                out.push(ContentToken {
                    kind: ContentKind::Operator(b"BI".to_vec()),
                    span: start..spanned.end,
                    value: Object::name("BI"),
                });
                out.push(ContentToken {
                    kind: ContentKind::Operand,
                    span: spanned.start..spanned.end,
                    value: object_of(other),
                });
                return spanned.end;
            }
        }
    }
}

/// An inline image's own token: its dictionary, its data, and **every byte it occupies**.
///
/// The span is the whole `BI … EI` region and not only its first byte. GOAL.md §4.1's seventh
/// law is that a selectable object knows exactly which bytes produced it, and a one-byte span is
/// a lie that only shows up when something writes back: an edit that inserted a wrapper at that
/// byte split the `BI` keyword in half, and the image vanished from the page with nothing
/// anywhere reporting it. Found by a corpus round trip.
fn push_image(out: &mut Vec<ContentToken>, start: usize, dict: Dict, data: Vec<u8>, end: usize) {
    out.push(ContentToken {
        kind: ContentKind::InlineImage { dict, data },
        span: start..end,
        value: Object::Null,
    });
}

/// Find the `EI` that ends an inline image's data.
///
/// The specification requires whitespace on both sides, and that is the only thing
/// separating image data from a literal `EI` inside a compressed scan. A two-byte `EI`
/// with whitespace around it is the best available rule; a file that embeds the bytes
/// ` EI ` in its data is unrecoverable by anyone.
fn find_ei(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 2 <= data.len() {
        if data.get(i) == Some(&b'E')
            && data.get(i + 1) == Some(&b'I')
            && i > from
            && data.get(i - 1).is_some_and(u8::is_ascii_whitespace)
            && data.get(i + 2).is_none_or(|b| b.is_ascii_whitespace())
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index a list whose length they
    // have just asserted; both are what a test is for. The panic-free rule is about what
    // the product does with a file, not about how a test reads one.
    // `assertions_on_constants` is allowed because three of the tests below assert that a
    // pair of constants is the pair the product intends. That is the whole content of those
    // assertions, and clippy's objection — that the outcome cannot change at run time — is
    // the reason the test exists rather than a reason against it.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::assertions_on_constants
    )]

    use super::*;

    fn ops(data: &[u8]) -> Vec<(String, Vec<f64>)> {
        ContentStream::parse(data)
            .operations()
            .into_iter()
            .map(|op| {
                (
                    String::from_utf8_lossy(op.operator.operator().unwrap_or_default())
                        .into_owned(),
                    op.reals(),
                )
            })
            .collect()
    }

    #[test]
    fn operands_and_operators_are_grouped() {
        assert_eq!(
            ops(b"1 0 0 1 10 20 cm /F1 12 Tf BT ET"),
            vec![
                ("cm".into(), vec![1.0, 0.0, 0.0, 1.0, 10.0, 20.0]),
                ("Tf".into(), vec![12.0]),
                ("BT".into(), vec![]),
                ("ET".into(), vec![]),
            ]
        );
    }

    #[test]
    fn a_real_and_an_integer_keep_their_spelling() {
        let c = ContentStream::parse(b".5 -.5 +3 -3");
        let values: Vec<Object> = c.tokens().iter().map(|t| t.value.clone()).collect();
        assert!(matches!(values.first(), Some(Object::Real(v)) if (*v - 0.5).abs() < 1e-9));
        assert!(matches!(values.get(1), Some(Object::Real(v)) if (*v + 0.5).abs() < 1e-9));
    }

    #[test]
    fn a_span_covers_exactly_its_token() {
        let c = ContentStream::parse(b"10 20 m");
        for t in c.tokens() {
            let bytes = c.bytes().get(t.span.clone()).unwrap();
            assert!(!bytes.is_empty());
            assert_eq!(bytes, t.bytes(c.bytes()));
        }
    }

    #[test]
    fn an_offset_finds_the_token_that_covers_it() {
        let c = ContentStream::parse(b"1 0 0 1 10 20 cm");
        // Spans are the token's own bytes: `10` occupies 8 and 9, and the space at 10
        // belongs to nothing.
        let t = c.token_at(8).expect("a token at 8");
        assert_eq!(t.value.as_i64(), Some(10));
        assert!(c.token_at(10).is_none(), "a space belongs to no token");
        let op = c.operation_at(15).expect("an operation at 15");
        assert_eq!(op.operator.operator(), Some(&b"cm"[..]));
    }

    #[test]
    fn a_comment_is_not_an_operand() {
        // A comment runs to the end of its line, so the `1 2 3` inside it are text and
        // the `m` on the next line is the operator.
        assert_eq!(
            ops(b"% 1 2 3\nm\nS"),
            vec![("m".into(), vec![]), ("S".into(), vec![])]
        );
    }

    #[test]
    fn a_dictionary_operand_is_kept() {
        let c = ContentStream::parse(b"<< /Type /X >> BDC");
        let op = c.operations().into_iter().next().expect("an operation");
        assert_eq!(op.operator.operator(), Some(&b"BDC"[..]));
        assert!(matches!(
            op.operands.first().map(|t| &t.value),
            Some(Object::Dict(_))
        ));
    }

    /// The array an operator takes is its contents, not a bracket: `TJ`'s kerning and
    /// `d`'s dash pattern are both arrays, and an empty one silently throws away the
    /// spacing a page was authored with.
    #[test]
    fn an_array_operand_keeps_its_items() {
        let c = ContentStream::parse(b"[(A) -250 (B) 500 (C)] TJ");
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Array(items)) = op.operands.first().map(|t| &t.value) else {
            panic!(
                "expected an array, got {:?}",
                op.operands.first().map(|t| &t.value)
            );
        };
        assert_eq!(items.len(), 5, "three strings and two kerns");
        assert!(matches!(items.first(), Some(Object::String(s)) if s == b"A"));
        assert!(matches!(items.get(1), Some(Object::Int(-250))));
        assert!(matches!(items.last(), Some(Object::String(s)) if s == b"C"));
    }

    #[test]
    fn a_dash_array_keeps_its_lengths() {
        let c = ContentStream::parse(b"[3 1] 2 d");
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Array(items)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected an array");
        };
        let numbers: Vec<f64> = items.iter().filter_map(Object::as_f64).collect();
        assert_eq!(numbers, vec![3.0, 1.0]);
    }

    #[test]
    fn an_array_spans_from_its_bracket_to_the_matching_close() {
        let source = b"[1 2 3] TJ";
        let c = ContentStream::parse(source);
        let t = c.tokens().first().expect("a token");
        assert_eq!(
            source.get(t.span.clone()),
            Some(&b"[1 2 3]"[..]),
            "the span covers the brackets, which is what an edit needs"
        );
    }

    #[test]
    fn a_dictionary_operand_keeps_its_entries() {
        let c = ContentStream::parse(b"<< /Type /OC /Name /Layer >> BDC");
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Dict(d)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected a dictionary");
        };
        assert_eq!(d.len(), 2, "two entries");
        assert_eq!(d.get("Type").and_then(Object::as_name), Some(&b"OC"[..]));
        assert_eq!(d.get("Name").and_then(Object::as_name), Some(&b"Layer"[..]));
    }

    #[test]
    fn a_double_close_bracket_does_not_leak_a_stray_token() {
        // `>>` is written as two closers; one is consumed by the dictionary and the other
        // must not become an operand of whatever comes next.
        let c = ContentStream::parse(b"<< /A 1 >> /Name BDC");
        let op = c.operations().into_iter().next().expect("an operation");
        assert_eq!(
            op.operands.len(),
            2,
            "the dictionary and the tag: {:?}",
            op.operands.len()
        );
        assert!(matches!(
            op.operands.first().map(|t| &t.value),
            Some(Object::Dict(_))
        ));
        assert!(
            matches!(op.operands.get(1).map(|t| &t.value), Some(Object::Name(n)) if n.as_bytes() == b"Name")
        );
    }

    #[test]
    fn an_unclosed_collection_ends_at_the_end_of_the_stream() {
        // A file that opens an array and stops. The array takes what it finds and the
        // stream does not hang.
        let c = ContentStream::parse(b"[(A) (B) 1 0 0 1");
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Array(items)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected an array");
        };
        assert!(items.len() >= 3, "it took what was there: {}", items.len());
    }

    #[test]
    fn an_empty_array_is_empty_rather_than_missing() {
        let c = ContentStream::parse(b"[] TJ");
        let op = c.operations().into_iter().next().expect("an operation");
        assert!(
            matches!(op.operands.first().map(|t| &t.value), Some(Object::Array(a)) if a.is_empty())
        );
    }

    #[test]
    fn an_inline_image_is_one_token() {
        let data = b"q BI /W 4 /H 4 /BPC 8 /CS /G ID \x00\x01\x02\x03\xff EI Q";
        let c = ContentStream::parse(data);
        let images: Vec<&ContentToken> = c
            .tokens()
            .iter()
            .filter(|t| matches!(t.kind, ContentKind::InlineImage { .. }))
            .collect();
        assert_eq!(
            images.len(),
            1,
            "the image is one thing, not a dozen tokens"
        );
        let ContentKind::InlineImage { dict, data: body } = &images[0].kind else {
            panic!("expected an image");
        };
        assert_eq!(dict.get("W").and_then(Object::as_i64), Some(4));
        assert_eq!(dict.get("CS").and_then(Object::as_name), Some(&b"G"[..]));
        // The specification excludes only the single whitespace byte after `ID`. The
        // whitespace before `EI` is data, and a decoder that silently drops it is
        // making a choice the format did not give it.
        assert_eq!(body.as_slice(), b"\x00\x01\x02\x03\xff ");
    }

    /// **The span of an inline image is every byte it occupies, not its first one.**
    ///
    /// GOAL.md §4.1's seventh law is that a selectable object knows exactly which bytes produced
    /// it, and write-back is what that law is *for*. A one-byte span passed every content test
    /// and then split a `BI` keyword in half the first time an edit inserted a wrapper in front
    /// of the image: the page lost the image and nothing reported it. Found by a corpus round
    /// trip through `mangle-edit`, not by anything here.
    #[test]
    fn an_inline_image_spans_every_byte_it_occupies() {
        let data = b"q BI /W 4 /H 4 ID \x00\x01\x02\x03\xff EI Q";
        let c = ContentStream::parse(data);
        let image = c
            .tokens()
            .iter()
            .find(|t| matches!(t.kind, ContentKind::InlineImage { .. }))
            .expect("the image");
        assert_eq!(
            image.span.start,
            data.iter().position(|b| *b == b'B').expect("BI"),
            "the span starts at the `BI`, not inside it"
        );
        // `BI` through `EI`: both the keyword and the data between them.
        let covered = data.get(image.span.clone()).unwrap_or_default();
        assert!(
            covered.starts_with(b"BI") && covered.ends_with(b"EI"),
            "the span covers the whole image, not one byte of it: {covered:?}"
        );
        assert!(
            covered.len() > 8,
            "and it is more than the keyword: {covered:?}"
        );
        // And the operation the interpreter runs carries the same span.
        let op = c
            .operation_at(image.span.start)
            .expect("an operation there");
        assert_eq!(
            op.span, image.span,
            "so an edit can address the whole image"
        );
    }

    #[test]
    fn an_inline_image_containing_operator_like_bytes_is_one_token() {
        // The data is deliberately full of things that would be operators.
        let data = b"BI /W 2 /H 2 ID \x0b\x0c m S n BT ET \xfd EI Q";
        let c = ContentStream::parse(data);
        let names: Vec<String> = c
            .tokens()
            .iter()
            .filter_map(|t| match &t.kind {
                ContentKind::Operator(n) => Some(String::from_utf8_lossy(n).into_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["Q"],
            "only the `Q` after the image is an operator"
        );
    }

    #[test]
    fn an_inline_image_with_no_end_runs_to_the_end() {
        let data = b"BI /W 2 /H 2 ID \x01\x02\x03\x04";
        let c = ContentStream::parse(data);
        let images: Vec<&ContentToken> = c
            .tokens()
            .iter()
            .filter(|t| matches!(t.kind, ContentKind::InlineImage { .. }))
            .collect();
        assert_eq!(images.len(), 1, "an unterminated image is still an image");
        let ContentKind::InlineImage { data: body, .. } = &images[0].kind else {
            panic!("expected an image");
        };
        assert_eq!(body.as_slice(), b"\x01\x02\x03\x04");
    }

    #[test]
    fn a_bare_bi_is_just_an_operator() {
        assert_eq!(
            ops(b"BI 1 2 m"),
            vec![("BI".into(), vec![]), ("m".into(), vec![1.0, 2.0])]
        );
    }

    #[test]
    fn trailing_operands_are_reported_rather_than_dropped() {
        let c = ContentStream::parse(b"S 1 2");
        let operations = c.operations();
        let last = operations.last().expect("an operation");
        assert_eq!(last.operands.len(), 2, "the stream ended with two operands");
    }

    #[test]
    fn the_operators_used_are_listed_once_each() {
        let c = ContentStream::parse(b"q 1 0 0 1 0 0 cm Q 1 0 0 1 0 0 cm Q");
        assert_eq!(
            c.operators(),
            vec![b"q".to_vec(), b"cm".to_vec(), b"Q".to_vec()]
        );
    }

    #[test]
    fn xobject_names_are_collected() {
        let c = ContentStream::parse(b"q /Im1 Do Q /Im2 Do /Im1 Do");
        assert_eq!(
            c.xobject_names(),
            vec![b"Im1".to_vec(), b"Im2".to_vec(), b"Im1".to_vec()]
        );
    }

    #[test]
    fn an_empty_stream_is_empty_rather_than_an_error() {
        let c = ContentStream::parse(b"");
        assert!(c.tokens().is_empty());
        assert!(c.operations().is_empty());
    }

    #[test]
    fn unrecognised_bytes_do_not_stop_the_stream() {
        // A stray `)` is legal garbage after damage; the operations after it still work.
        let c = ContentStream::parse(b"1 0 0 1 0 0 cm ) 2 0 0 1 5 5 cm");
        let names: Vec<String> = c
            .tokens()
            .iter()
            .filter_map(|t| match &t.kind {
                ContentKind::Operator(n) => Some(String::from_utf8_lossy(n).into_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["cm", "cm"]);
    }

    // ── Two bounds, not one ──────────────────────────────────────────────────────
    //
    // `MAX_COLLECTION_DEPTH` is how deep a collection may nest. `MAX_COLLECTION_ITEMS` is
    // how many entries one may hold. They were one number, and a `TJ` array of more than
    // thirty-two elements lost everything past the thirty-second with nothing to say so.

    /// The operand of the first operation, as an array.
    fn first_array(data: &[u8]) -> Vec<Object> {
        let c = ContentStream::parse(data);
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Array(items)) = op.operands.first().map(|t| &t.value) else {
            panic!(
                "expected an array, got {:?}",
                op.operands.first().map(|t| &t.value)
            );
        };
        items.clone()
    }

    /// How many items a value holds, counting every collection inside it.
    fn items_inside(value: &Object) -> usize {
        match value {
            Object::Array(items) => items.len() + items.iter().map(items_inside).sum::<usize>(),
            Object::Dict(d) => d.iter().map(|(_, v)| 1 + items_inside(v)).sum(),
            _ => 0,
        }
    }

    /// An array of `n` one-character strings, kerned after each but the last.
    ///
    /// This is the shape a typesetter's `TJ` actually is: one string per character and one
    /// number between each pair, so `n` characters is `2n − 1` items.
    fn kerned_show(n: usize) -> (Vec<u8>, Vec<Object>) {
        let mut bytes = vec![b'['];
        let mut expected = Vec::new();
        for i in 0..n {
            let letter = (b'A' + (i % 26) as u8) as char;
            bytes.extend_from_slice(format!("({letter})").as_bytes());
            expected.push(Object::string(&letter.to_string()));
            if i + 1 < n {
                let kern = -(10 + i as i64);
                bytes.extend_from_slice(kern.to_string().as_bytes());
                expected.push(Object::Int(kern));
            }
        }
        bytes.extend_from_slice(b"]TJ");
        (bytes, expected)
    }

    /// The items in a `TJ` array carrying one full line of ordinary text.
    ///
    /// A full line of 12-point text in a 600-point measure is about a hundred characters, at a
    /// mean advance of half an em, and a typesetter's `TJ` kerns between each pair — so two
    /// items per character, `2 × 100 − 1`. **This number is written out rather than taken from
    /// [`MAX_COLLECTION_ITEMS`]**, because a test whose expectation comes from the constant it
    /// is testing proves nothing: with the bound at thirty-two it would have agreed.
    const A_LINE_OF_TEXT: usize = 199;

    /// `0` through `n - 1` written as array items.
    fn counting_array(n: usize) -> Vec<u8> {
        let mut bytes = vec![b'['];
        for i in 0..n {
            bytes.extend_from_slice(format!("{i} ").as_bytes());
        }
        bytes.extend_from_slice(b"]TJ");
        bytes
    }

    #[test]
    fn an_array_of_a_hundred_items_keeps_all_of_them() {
        let items = first_array(&counting_array(100));
        assert_eq!(items.len(), 100, "a hundred items is not a bound to reach");
        let numbers: Vec<Option<i64>> = items.iter().map(Object::as_i64).collect();
        let expected: Vec<Option<i64>> = (0..100).map(|i| Some(i64::from(i))).collect();
        assert_eq!(numbers, expected, "each item is the value at its own index");
    }

    #[test]
    fn a_kerned_show_of_eighty_four_strings_keeps_every_string_and_every_kern() {
        // The array that made the defect visible: one form XObject on a corpus page
        // carrying eighty-four strings, written with no whitespace between the elements,
        // which the old bound cut at the thirty-second item — sixteen strings and sixteen
        // kerns, and the rest of the line gone mid-word.
        let (bytes, expected) = kerned_show(84);
        let items = first_array(&bytes);
        assert_eq!(
            items.len(),
            167,
            "eighty-four strings and the eighty-three kerns between them"
        );
        assert!(
            MAX_COLLECTION_ITEMS > MAX_COLLECTION_DEPTH,
            "the two bounds must be different questions, and an item bound above one line of \
             text — {A_LINE_OF_TEXT} items. They were one number, at 32, which is why this file \
             was truncated"
        );
        assert_eq!(items, expected, "every string and every kern, in order");
        let strings: Vec<&Vec<u8>> = items
            .iter()
            .filter_map(|i| match i {
                Object::String(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(
            strings.len(),
            84,
            "which is the count the corpus page was missing"
        );
        let kerns: Vec<i64> = items
            .iter()
            .filter_map(|i| match i {
                Object::Int(v) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(kerns.len(), 83, "and one kern between each pair of strings");
        assert!(
            kerns.iter().all(|k| *k < 0),
            "a kern is a negative displacement: {kerns:?}"
        );
    }

    #[test]
    fn a_dictionary_of_a_hundred_entries_keeps_all_of_them() {
        let mut bytes = vec![b'<', b'<'];
        for i in 0..100 {
            bytes.extend_from_slice(format!("/K{i} {i} ").as_bytes());
        }
        bytes.extend_from_slice(b">>BDC");
        let c = ContentStream::parse(&bytes);
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Dict(d)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected a dictionary");
        };
        assert_eq!(d.len(), 100, "a hundred entries is not a bound to reach");
        for i in 0..100 {
            assert_eq!(
                d.get(&format!("K{i}")).and_then(Object::as_i64),
                Some(i64::from(i)),
                "entry K{i} kept its own value"
            );
        }
    }

    #[test]
    fn brackets_never_closed_stop_at_the_depth_bound_and_are_reported() {
        // A hundred thousand `[` and not one `]`. This is the case the depth bound is
        // for, and it is the test that a fix which only raised the item bound, or raised
        // the depth bound, would fail: the parse must finish, must hold a bounded number
        // of items, and must say that it stopped.
        let bytes = vec![b'['; 100_000];
        let c = ContentStream::parse(&bytes);
        assert_eq!(
            c.notes().len(),
            1,
            "one report, not one per level: {:?}",
            c.notes()
        );
        let note = c.notes().first().expect("a note");
        assert!(
            note.contains("nests deeper than the 32 levels"),
            "which says it was the nesting that stopped it, at thirty-two, and not the item \
             count: {note}"
        );
        assert!(note.contains("byte"), "and where: {note}");

        // What it held is thirty-one items: one empty collection at each level above the
        // innermost, and nothing else. **Written out rather than taken from
        // `MAX_COLLECTION_DEPTH`**, because this is the test that catches the wrong fix —
        // raising the one constant so that arrays stop being truncated — and a test whose
        // expectation comes from the constant it is testing would follow the constant up.
        let op = c.operations().into_iter().next().expect("an operation");
        let value = op.operands.first().map(|t| &t.value).expect("an operand");
        assert!(
            matches!(value, Object::Array(items) if items.len() == 1),
            "the outermost array kept the one collection it had already read"
        );
        assert_eq!(
            items_inside(value),
            31,
            "a hundred thousand unclosed brackets are worth thirty-one items and no more"
        );
    }

    #[test]
    fn an_array_over_the_item_bound_is_cut_short_and_reported_by_name() {
        let c = ContentStream::parse(&counting_array(MAX_COLLECTION_ITEMS + 500));
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Array(items)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected an array");
        };
        assert_eq!(
            items.len(),
            MAX_COLLECTION_ITEMS,
            "it stops at the bound rather than growing"
        );
        assert_eq!(
            items.last().and_then(Object::as_i64),
            Some(MAX_COLLECTION_ITEMS as i64 - 1),
            "and it kept the first items, not the last"
        );
        let kept: Vec<Option<i64>> = items.iter().map(Object::as_i64).collect();
        let expected: Vec<Option<i64>> = (0..MAX_COLLECTION_ITEMS)
            .map(|i| Some(i64::from(i32::try_from(i).expect("the bound is below i32"))))
            .collect();
        assert_eq!(
            kept, expected,
            "every item from zero to the bound, so what was kept is the beginning of the array"
        );
        assert_eq!(c.notes().len(), 1, "one report: {:?}", c.notes());
        let note = c.notes().first().expect("a note");
        assert!(
            MAX_COLLECTION_ITEMS > MAX_COLLECTION_DEPTH,
            "and a bound for the items that is not the bound for the nesting"
        );
        assert!(note.contains("array"), "which names what it cut: {note}");
        assert!(
            note.contains(&MAX_COLLECTION_ITEMS.to_string()),
            "and the count it reached: {note}"
        );
    }

    #[test]
    fn an_array_exactly_at_the_item_bound_is_kept_and_one_more_is_not() {
        // The off-by-one lives here, on both sides at once: the bound itself is legal and
        // the bound plus one is not.
        let exact = ContentStream::parse(&counting_array(MAX_COLLECTION_ITEMS));
        let op = exact.operations().into_iter().next().expect("an operation");
        assert!(
            matches!(op.operands.first().map(|t| &t.value), Some(Object::Array(a)) if a.len() == MAX_COLLECTION_ITEMS),
            "an array of exactly {MAX_COLLECTION_ITEMS} items is accepted"
        );
        assert!(
            MAX_COLLECTION_ITEMS > MAX_COLLECTION_DEPTH,
            "and the bound being tested has to be the item bound rather than the depth bound \
             again, or the boundary being tested is the wrong one"
        );
        assert!(
            exact.notes().is_empty(),
            "and nothing is reported for it: {:?}",
            exact.notes()
        );

        let over = ContentStream::parse(&counting_array(MAX_COLLECTION_ITEMS + 1));
        let op = over.operations().into_iter().next().expect("an operation");
        assert!(
            matches!(op.operands.first().map(|t| &t.value), Some(Object::Array(a)) if a.len() == MAX_COLLECTION_ITEMS),
            "one item more is cut back to the bound"
        );
        assert_eq!(
            over.notes().len(),
            1,
            "and that is reported: {:?}",
            over.notes()
        );
    }

    #[test]
    fn a_deeply_nested_but_small_document_still_parses() {
        // Depth and item count are two questions. A document nested to the bound and
        // holding one item at each level is nothing like one hundred thousand items, and
        // the bound that stops the second must not touch the first.
        let mut bytes = vec![b'['; MAX_COLLECTION_DEPTH];
        bytes.extend_from_slice(b"42");
        bytes.extend(std::iter::repeat_n(b']', MAX_COLLECTION_DEPTH));
        bytes.extend_from_slice(b"TJ");
        let c = ContentStream::parse(&bytes);
        assert!(
            c.notes().is_empty(),
            "nesting to the bound is legal and is not a finding: {:?}",
            c.notes()
        );

        let op = c.operations().into_iter().next().expect("an operation");
        let mut value = op.operands.first().map(|t| &t.value).expect("an operand");
        let mut levels = 0usize;
        while let Object::Array(items) = value {
            assert_eq!(items.len(), 1, "one item at each level");
            value = items.first().expect("the item");
            levels += 1;
        }
        assert_eq!(
            levels, MAX_COLLECTION_DEPTH,
            "every level was read, all {MAX_COLLECTION_DEPTH} of them"
        );
        assert_eq!(
            value.as_i64(),
            Some(42),
            "and the item at the bottom of them came through"
        );
    }

    #[test]
    fn a_nested_collection_keeps_its_own_items() {
        // An optional-content membership list inside a marked-content property list is
        // the deepest thing a real content stream holds, and it has to arrive as what it
        // is rather than as its parts.
        let c = ContentStream::parse(b"<< /Type /OCMD /OCGs [/OC1 /OC2] >> BDC");
        let op = c.operations().into_iter().next().expect("an operation");
        let Some(Object::Dict(d)) = op.operands.first().map(|t| &t.value) else {
            panic!("expected a dictionary");
        };
        assert_eq!(d.len(), 2, "two entries");
        let Some(Object::Array(members)) = d.get("OCGs") else {
            panic!("the membership list is an array");
        };
        assert_eq!(
            members.len(),
            2,
            "with both of its names, not with the names and the brackets mixed in"
        );
        assert!(matches!(members.last(), Some(Object::Name(n)) if n.as_bytes() == b"OC2"));
    }
}
