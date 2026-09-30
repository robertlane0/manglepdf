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
        }
    }

    /// The bytes this stream was parsed from.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.source
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
                    push_image(out, start, dict, body);
                    return data.len();
                };
                let body = data.get(body_start..body_end).unwrap_or_default().to_vec();
                push_image(out, start, dict, body);
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

fn push_image(out: &mut Vec<ContentToken>, start: usize, dict: Dict, data: Vec<u8>) {
    out.push(ContentToken {
        kind: ContentKind::InlineImage { dict, data },
        span: start..start + 1,
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
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
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
}
