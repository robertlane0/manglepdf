//! The object parser: tokens to [`Object`], and streams with their raw bytes.

use crate::error::{Error, Result};
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, Name, Object, Ref, Stream};

/// Recursive-descent parser over a byte slice.
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    /// Nesting limit, so a hostile file cannot exhaust the stack.
    depth: usize,
    /// Hard limit on the number of objects parsed from one construct.
    budget: usize,
}

/// The nesting and size limits every parse runs under.
pub const MAX_DEPTH: usize = 300;
/// An array or dictionary deeper than this many elements is truncated.
pub const MAX_ELEMENTS: usize = 200_000;

impl<'a> Parser<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            lexer: Lexer::new(data),
            depth: 0,
            budget: 4 * MAX_ELEMENTS,
        }
    }

    #[must_use]
    pub fn at(data: &'a [u8], offset: usize) -> Self {
        let mut p = Self::new(data);
        p.lexer.seek(offset);
        p
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.lexer.position()
    }

    /// The next object, or `None` at end of input.
    pub fn next_object(&mut self) -> Result<Option<Object>> {
        self.budget = self.budget.saturating_sub(1);
        if self.budget == 0 {
            return Err(Error::Limit {
                kind: "object count",
                limit: 4 * MAX_ELEMENTS,
            });
        }
        self.lexer.skip_space();
        if self.lexer.at_end() {
            return Ok(None);
        }
        // Look ahead: `int int R` is a reference.
        if let Some(obj) = self.try_reference()? {
            return Ok(Some(obj));
        }
        let t = self.lexer.next_token()?;
        let obj = match t.token {
            Token::Null => Object::Null,
            Token::Bool(b) => Object::Bool(b),
            Token::Int(i) => Object::Int(i),
            Token::Real(r) => Object::Real(r),
            Token::String(s) | Token::HexString(s) => Object::String(s),
            Token::Name(n) => Object::Name(Name(n)),
            Token::ArrayOpen => Object::Array(self.parse_array()?),
            Token::DictOpen => Object::Dict(self.parse_dict()?),
            Token::Keyword(k) => {
                return Err(Error::at(
                    t.start,
                    &format!("unexpected keyword `{}`", String::from_utf8_lossy(&k)),
                ));
            }
            Token::Eof => return Ok(None),
            Token::ArrayClose | Token::DictClose | Token::BraceOpen | Token::BraceClose => {
                return Err(Error::at(t.start, "unexpected delimiter"));
            }
        };
        Ok(Some(obj))
    }

    /// Like [`Parser::next_object`] but treats end of input as a syntax error.
    pub fn expect_object(&mut self) -> Result<Object> {
        self.next_object()?
            .ok_or_else(|| Error::at(self.position(), "unexpected end of input"))
    }

    /// Consume the next token and check it is the given keyword.
    pub fn next_keyword(&mut self, kw: &[u8]) -> bool {
        let save = self.lexer.position();
        match self.lexer.next_token() {
            Ok(t) if t.is_keyword(kw) => true,
            _ => {
                self.lexer.seek(save);
                false
            }
        }
    }

    /// `num generation R` is a reference; anything else is left for the caller.
    fn try_reference(&mut self) -> Result<Option<Object>> {
        let save = self.lexer.position();
        let first = self.lexer.next_token()?;
        let Token::Int(num) = first.token else {
            self.lexer.seek(save);
            return Ok(None);
        };
        if num < 0 || num > i64::from(u32::MAX) {
            self.lexer.seek(save);
            return Ok(None);
        }
        let save2 = self.lexer.position();
        let second = self.lexer.next_token()?;
        let Token::Int(generation) = second.token else {
            self.lexer.seek(save);
            return Ok(None);
        };
        if !(0..=i64::from(u16::MAX)).contains(&generation) {
            self.lexer.seek(save);
            return Ok(None);
        }
        let third = self.lexer.next_token()?;
        if third.is_keyword(b"R") {
            return Ok(Some(Object::Ref(Ref::new(
                u32::try_from(num).unwrap_or(0),
                u16::try_from(generation).unwrap_or(0),
            ))));
        }
        self.lexer.seek(save2);
        self.lexer.seek(save);
        Ok(None)
    }

    fn parse_array(&mut self) -> Result<Vec<Object>> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(Error::Limit {
                kind: "nesting depth",
                limit: MAX_DEPTH,
            });
        }
        let mut out = Vec::new();
        loop {
            self.lexer.skip_space();
            if self.lexer.at_end() {
                self.depth -= 1;
                return Err(Error::at(self.position(), "unterminated array"));
            }
            let t = self.lexer.next_token()?;
            match t.token {
                Token::ArrayClose => break,
                Token::Eof => {
                    self.depth -= 1;
                    return Err(Error::at(t.start, "unterminated array"));
                }
                Token::DictClose | Token::BraceOpen | Token::BraceClose => {
                    self.lexer.seek(t.start);
                }
                _ => {
                    self.lexer.seek(t.start);
                    if out.len() >= MAX_ELEMENTS {
                        self.depth -= 1;
                        return Err(Error::Limit {
                            kind: "array length",
                            limit: MAX_ELEMENTS,
                        });
                    }
                    match self.next_object()? {
                        Some(o) => out.push(o),
                        None => break,
                    }
                }
            }
        }
        self.depth -= 1;
        Ok(out)
    }

    fn parse_dict(&mut self) -> Result<Dict> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(Error::Limit {
                kind: "nesting depth",
                limit: MAX_DEPTH,
            });
        }
        let mut d = Dict::new();
        loop {
            self.lexer.skip_space();
            if self.lexer.at_end() {
                self.depth -= 1;
                return Err(Error::at(self.position(), "unterminated dictionary"));
            }
            let t = self.lexer.next_token()?;
            match t.token {
                Token::DictClose => break,
                Token::Eof => {
                    self.depth -= 1;
                    return Err(Error::at(t.start, "unterminated dictionary"));
                }
                Token::Name(key) => {
                    self.lexer.skip_space();
                    if self.lexer.peek_byte() == Some(b'>') && self.lexer.position() > 0 {
                        // `>>` closes the dictionary instead of ending the value.
                        self.lexer.seek(t.start);
                        self.depth -= 1;
                        return Err(Error::at(t.start, "dictionary key with no value"));
                    }
                    let value = self
                        .next_object()?
                        .ok_or_else(|| Error::at(t.start, "dictionary key with no value"))?;
                    d.insert(Name(key), value);
                }
                _ => {
                    // Skip anything that is not a key, so one bad entry does not lose
                    // the whole dictionary.
                    if d.is_empty() && matches!(t.token, Token::Keyword(_)) {
                        continue;
                    }
                    return Err(Error::at(t.start, "expected a name in dictionary"));
                }
            }
        }
        d.build_index();
        self.depth -= 1;
        Ok(d)
    }

    /// Parse a stream body. `data` is the whole file; `dict_start` is where the
    /// dictionary began, and `len_hint` is the `/Length` if it was an integer.
    ///
    /// Returns the dictionary, the raw bytes, and the offset just past `endstream`.
    pub fn parse_stream(
        data: &'a [u8],
        dict_start: usize,
        len_hint: Option<i64>,
    ) -> Result<(Dict, Vec<u8>, usize)> {
        let mut p = Parser::at(data, dict_start);
        let Some(Object::Dict(dict)) = p.next_object()? else {
            return Err(Error::at(dict_start, "expected a stream dictionary"));
        };
        p.lexer.skip_space();
        let mut i = p.position();
        // The keyword, then CRLF or LF; a lone CR is tolerated.
        if data.get(i..i + 6) != Some(b"stream") {
            return Err(Error::at(i, "expected `stream`"));
        }
        i += 6;
        if data.get(i) == Some(&b'\r') {
            i += 1;
        }
        if data.get(i) == Some(&b'\n') {
            i += 1;
        }
        let body_start = i;

        // `/Length` may be an indirect reference, in which case we must scan.
        let (raw, end) = if let Some(n) = len_hint
            && n >= 0
        {
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            let candidate_end = body_start.saturating_add(n);
            let ok = data.get(candidate_end..).is_some_and(endstream_follows_at);
            match data.get(body_start..candidate_end) {
                Some(body) if ok => (body.to_vec(), candidate_end),
                _ => scan_for_endstream(data, body_start),
            }
        } else {
            scan_for_endstream(data, body_start)
        };

        // Skip to just past `endstream`.
        let mut j = end;
        while let Some(&b) = data.get(j) {
            if b.is_ascii_whitespace() {
                j += 1;
            } else {
                break;
            }
        }
        if data.get(j..j.saturating_add(9)) == Some(b"endstream".as_slice()) {
            j += 9;
        }
        Ok((dict, raw, j))
    }
}

/// Does `endstream` follow, allowing whitespace and NULs between the data and it?
fn endstream_follows_at(tail: &[u8]) -> bool {
    // Cap the skip: a megabyte of whitespace is not a length we should trust.
    const MAX_SKIP: usize = 4;
    let mut i = 0usize;
    while i < MAX_SKIP {
        match tail.get(i) {
            Some(b) if b.is_ascii_whitespace() || *b == 0 => i += 1,
            _ => break,
        }
    }
    tail.get(i..).is_some_and(|t| t.starts_with(b"endstream"))
}

/// Find `endstream` by scanning, for when `/Length` lies or is an indirect reference.
fn scan_for_endstream(data: &[u8], from: usize) -> (Vec<u8>, usize) {
    let mut i = from;
    while i + 9 <= data.len() {
        if data.get(i..i + 9) == Some(b"endstream".as_slice()) {
            // The keyword is preceded by an EOL, which is not part of the data.
            let mut end = i;
            if end > from && data.get(end - 1) == Some(&b'\n') {
                end -= 1;
                if end > from && data.get(end - 1) == Some(&b'\r') {
                    end -= 1;
                }
            }
            return (data.get(from..end).unwrap_or(&[]).to_vec(), end);
        }
        i += 1;
    }
    // No terminator: the stream runs to the end of the file.
    (data.get(from..).unwrap_or(&[]).to_vec(), data.len())
}

/// Build a stream object from an already-parsed dictionary and body.
#[must_use]
pub fn make_stream(dict: Dict, raw: Vec<u8>, offset: Option<usize>) -> Stream {
    Stream {
        dict,
        raw,
        file_offset: offset,
        synthetic: false,
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn parse(s: &[u8]) -> Object {
        Parser::new(s)
            .next_object()
            .expect("parse")
            .expect("object")
    }

    #[test]
    fn scalars() {
        assert_eq!(parse(b"42"), Object::Int(42));
        assert_eq!(parse(b" true "), Object::Bool(true));
        assert_eq!(parse(b"/Name"), Object::Name(Name::new("Name")));
        assert_eq!(parse(b"(s)"), Object::String(b"s".to_vec()));
        assert_eq!(parse(b"[]"), Object::Array(vec![]));
    }

    #[test]
    fn references() {
        assert_eq!(parse(b"12 0 R"), Object::Ref(Ref::new(12, 0)));
        assert_eq!(parse(b"12 3 R"), Object::Ref(Ref::new(12, 3)));
        // Not a reference: three separate tokens.
        assert_eq!(parse(b"12 3"), Object::Int(12));
    }

    #[test]
    fn dicts_and_arrays() {
        let o = parse(b"<< /Type /Page /Kids [1 0 R 2 0 R] /Count 2 >>");
        let d = o.as_dict().expect("dict");
        assert_eq!(d.get("Type").and_then(Object::as_name), Some(&b"Page"[..]));
        assert_eq!(
            d.get("Kids")
                .and_then(Object::as_array)
                .map(<[Object]>::len),
            Some(2)
        );
        assert_eq!(d.get("Count").and_then(Object::as_i64), Some(2));
    }

    #[test]
    fn nested() {
        let o = parse(b"[[1 2] << /A [3] >>]");
        let a = o.as_array().expect("array");
        assert_eq!(a.len(), 2);
        assert_eq!(
            a.get(1)
                .and_then(|o| o.get("A"))
                .and_then(Object::as_array)
                .map(<[Object]>::len),
            Some(1)
        );
    }

    #[test]
    fn stream_with_correct_length() {
        let data = b"<< /Length 5 >>\nstream\nHELLO\nendstream";
        let (d, raw, _) = Parser::parse_stream(data, 0, Some(5)).expect("stream");
        assert_eq!(d.get("Length").and_then(Object::as_i64), Some(5));
        assert_eq!(raw, b"HELLO");
    }

    #[test]
    fn stream_with_wrong_length_falls_back_to_scanning() {
        let data = b"<< /Length 99 >>\nstream\nHELLO\nendstream";
        let (_, raw, _) = Parser::parse_stream(data, 0, Some(99)).expect("stream");
        assert_eq!(raw, b"HELLO");
    }

    #[test]
    fn stream_with_binary_payload() {
        let payload: &[u8] = &[0, 255, b'e', b'n', b'd', b's', b't', b'r', b'e', b'a', b'm'];
        let mut data = b"<< /Length 11 >>\nstream\n".to_vec();
        data.extend_from_slice(payload);
        data.extend_from_slice(b"\nendstream");
        let (_, raw, _) = Parser::parse_stream(&data, 0, Some(11)).expect("stream");
        assert_eq!(raw, payload);
    }

    #[test]
    fn depth_limit() {
        let mut s = vec![b'['; MAX_DEPTH + 10];
        s.extend(std::iter::repeat_n(b']', MAX_DEPTH + 10));
        let r = Parser::new(&s).next_object();
        assert!(matches!(r, Err(Error::Limit { .. })), "{r:?}");
    }

    #[test]
    fn unterminated_is_an_error_not_a_hang() {
        assert!(Parser::new(b"[1 2 3").next_object().is_err());
        assert!(Parser::new(b"<< /A 1").next_object().is_err());
        assert!(Parser::new(b"(abc").next_object().is_err());
    }
}
