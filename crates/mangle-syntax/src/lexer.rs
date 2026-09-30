//! The PDF lexer.
//!
//! Deliberately hand-written and byte-exact: every token records the span of bytes it
//! came from, because the whole surgical write-back story depends on knowing precisely
//! which bytes produced an operator.

use crate::error::{Error, Result};

/// One lexical token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// A literal string, with escapes already resolved.
    String(Vec<u8>),
    /// A hex string.
    HexString(Vec<u8>),
    /// A name, with `#xx` escapes already resolved.
    Name(Vec<u8>),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
    /// A `{ ... }` PostScript function body, kept verbatim.
    BraceOpen,
    BraceClose,
    /// An indirect object reference such as `12 0 R`.
    Keyword(Vec<u8>),
    Eof,
}

impl Token {
    #[must_use]
    pub fn is_white_or_delimiter(b: u8) -> bool {
        matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'(' | b')' | b'<' | b'>'
            | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
    }

    #[must_use]
    pub fn is_regular(b: u8) -> bool {
        !Self::is_white_or_delimiter(b)
    }
}

/// A token together with the bytes it occupies.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub start: usize,
    pub end: usize,
}

impl Spanned {
    #[must_use]
    pub fn is_keyword(&self, kw: &[u8]) -> bool {
        matches!(&self.token, Token::Keyword(k) if k == kw)
    }
}

/// Streaming lexer over a byte slice.
#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    /// True at the end of the input.
    #[must_use]
    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Skip whitespace and comments. Returns `true` if a comment was skipped.
    pub fn skip_space(&mut self) -> bool {
        let mut saw_comment = false;
        while let Some(&b) = self.data.get(self.pos) {
            if b.is_ascii_whitespace() || b == b'\0' {
                self.pos += 1;
            } else if b == b'%' {
                saw_comment = true;
                while let Some(&c) = self.data.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        saw_comment
    }

    /// Peek at the next byte without consuming, skipping nothing.
    #[must_use]
    pub fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// The next token with its byte span. Comments are skipped.
    pub fn next_token(&mut self) -> Result<Spanned> {
        self.skip_space();
        let start = self.pos;
        let Some(&b) = self.data.get(self.pos) else {
            return Ok(Spanned {
                token: Token::Eof,
                start,
                end: start,
            });
        };
        let token = match b {
            b'[' => {
                self.pos += 1;
                Token::ArrayOpen
            }
            b']' => {
                self.pos += 1;
                Token::ArrayClose
            }
            b'{' => {
                self.pos += 1;
                Token::BraceOpen
            }
            b'}' => {
                self.pos += 1;
                Token::BraceClose
            }
            b'/' => {
                self.pos += 1;
                Token::Name(self.read_name_body())
            }
            b'(' => {
                self.pos += 1;
                Token::String(self.read_literal_string()?)
            }
            b'<' => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Token::DictOpen
                } else {
                    self.pos += 1;
                    Token::HexString(self.read_hex_string())
                }
            }
            b'>' => {
                if self.data.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Token::DictClose
                } else {
                    // A lone `>` is illegal; skip it so parsing can continue.
                    self.pos += 1;
                    return self.next_token();
                }
            }
            b')' => {
                // Unbalanced; skip.
                self.pos += 1;
                return self.next_token();
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.read_number()?,
            _ => self.read_keyword(),
        };
        Ok(Spanned {
            token,
            start,
            end: self.pos,
        })
    }

    fn read_name_body(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(&b) = self.data.get(self.pos) {
            if !Token::is_regular(b) {
                break;
            }
            if b == b'#' {
                let hi = self.data.get(self.pos + 1).copied().and_then(hex_val);
                let lo = self.data.get(self.pos + 2).copied().and_then(hex_val);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h << 4) | l);
                    self.pos += 3;
                    continue;
                }
            }
            out.push(b);
            self.pos += 1;
        }
        out
    }

    fn read_literal_string(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(&b) = self.data.get(self.pos) {
            self.pos += 1;
            match b {
                b'\\' => {
                    let Some(&e) = self.data.get(self.pos) else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        b'\r' => {
                            // Line continuation; swallow an optional \n.
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            // Up to three octal digits.
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                let Some(&d) = self.data.get(self.pos) else { break };
                                if !(b'0'..=b'7').contains(&d) {
                                    break;
                                }
                                v = v * 8 + u32::from(d - b'0');
                                self.pos += 1;
                            }
                            out.push(u8::try_from(v & 0xff).unwrap_or(0));
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b'(');
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(b')');
                }
                _ => out.push(b),
            }
        }
        if depth > 0 {
            return Err(Error::at(self.pos, "unterminated string"));
        }
        Ok(out)
    }

    fn read_hex_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while let Some(&b) = self.data.get(self.pos) {
            if b == b'>' {
                self.pos += 1;
                break;
            }
            self.pos += 1;
            if let Some(v) = hex_val(b) {
                if let Some(h) = high.take() {
                    out.push((h << 4) | v);
                } else {
                    high = Some(v);
                }
            }
            // Everything else, including whitespace, is ignored.
        }
        if let Some(h) = high {
            out.push(h << 4);
        }
        out
    }

    fn read_number(&mut self) -> Result<Token> {
        let start = self.pos;
        let mut is_real = false;
        if matches!(self.data.get(self.pos), Some(b'+') | Some(b'-')) {
            self.pos += 1;
        }
        while let Some(&b) = self.data.get(self.pos) {
            match b {
                b'0'..=b'9' => self.pos += 1,
                b'.' => {
                    is_real = true;
                    self.pos += 1;
                }
                b'-' | b'+' => {
                    // Only valid directly after an exponent marker; a second sign is
                    // junk, so stop and let the caller cope.
                    let prev = self.data.get(self.pos.wrapping_sub(1)).copied();
                    if matches!(prev, Some(b'e') | Some(b'E')) {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                b'e' | b'E' => {
                    // An exponent makes it a real, but `-` alone after `e` is junk.
                    let next = self.data.get(self.pos + 1).copied();
                    let after = self.data.get(self.pos + 2).copied();
                    if matches!(next, Some(b'0'..=b'9') | Some(b'+') | Some(b'-'))
                        && !matches!(after, Some(b'+') | Some(b'-'))
                    {
                        is_real = true;
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        let text = self.data.get(start..self.pos).unwrap_or(&[]);
        if text.is_empty() {
            return Err(Error::at(start, "expected a number"));
        }
        // Strip a leading `+`, which Rust's parser does not accept.
        let cleaned = text.strip_prefix(b'+').unwrap_or(text);
        let as_str = String::from_utf8_lossy(cleaned);
        if !is_real {
            if let Ok(i) = as_str.parse::<i64>() {
                return Ok(Token::Int(i));
            }
            // Out of i64 range: keep it as a real so the document still opens.
        }
        match as_str.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(Token::Real(v)),
            _ => Err(Error::at(start, "number is not finite")),
        }
    }

    fn read_keyword(&mut self) -> Token {
        let start = self.pos;
        while let Some(&b) = self.data.get(self.pos) {
            if !Token::is_regular(b) {
                break;
            }
            self.pos += 1;
        }
        let word = self.data.get(start..self.pos).unwrap_or(&[]);
        match word {
            b"true" => Token::Bool(true),
            b"false" => Token::Bool(false),
            b"null" => Token::Null,
            other => Token::Keyword(other.to_vec()),
        }
    }
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(data: &[u8]) -> Vec<Token> {
        let mut lx = Lexer::new(data);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token().expect("token");
            if t.token == Token::Eof {
                break;
            }
            out.push(t.token);
        }
        out
    }

    #[test]
    fn basics() {
        assert_eq!(
            toks(b"null true false 12 -3 4.5 -0.5"),
            vec![
                Token::Null,
                Token::Bool(true),
                Token::Bool(false),
                Token::Int(12),
                Token::Int(-3),
                Token::Real(4.5),
                Token::Real(-0.5),
            ]
        );
    }

    #[test]
    fn delimiters() {
        assert_eq!(
            toks(b"[ ] << >> { }"),
            vec![
                Token::ArrayOpen,
                Token::ArrayClose,
                Token::DictOpen,
                Token::DictClose,
                Token::BraceOpen,
                Token::BraceClose,
            ]
        );
    }

    #[test]
    fn names_with_escapes() {
        assert_eq!(toks(b"/Name"), vec![Token::Name(b"Name".to_vec())]);
        assert_eq!(toks(b"/A#20B"), vec![Token::Name(b"A B".to_vec())]);
        assert_eq!(toks(b"/A#"), vec![Token::Name(b"A#".to_vec())]);
        assert_eq!(toks(b"/"), vec![Token::Name(Vec::new())]);
    }

    #[test]
    fn literal_strings() {
        assert_eq!(toks(b"(hi)"), vec![Token::String(b"hi".to_vec())]);
        assert_eq!(toks(b"(a(b)c)"), vec![Token::String(b"a(b)c".to_vec())]);
        assert_eq!(toks(br"(a\)b)"), vec![Token::String(b"a)b".to_vec())]);
        assert_eq!(toks(br"(\101\102)"), vec![Token::String(b"AB".to_vec())]);
        assert_eq!(toks(b"(a\\\nb)"), vec![Token::String(b"ab".to_vec())]);
        assert_eq!(toks(b"(\n\t\r\b\f)"), {
            vec![Token::String(vec![b'\n', b'\t', b'\r', 0x08, 0x0c])]
        });
    }

    #[test]
    fn hex_strings() {
        assert_eq!(toks(b"<48656C6C6F>"), vec![Token::HexString(b"Hello".to_vec())]);
        assert_eq!(toks(b"<48 65 6c>"), vec![Token::HexString(b"Hel".to_vec())]);
        assert_eq!(toks(b"<4>"), vec![Token::HexString(vec![0x40])]);
        assert_eq!(toks(b"<4"), vec![Token::HexString(vec![0x40])]);
    }

    #[test]
    fn numbers() {
        assert_eq!(toks(b"+42"), vec![Token::Int(42)]);
        assert_eq!(toks(b"-.5"), vec![Token::Real(-0.5)]);
        assert_eq!(toks(b"1e3"), vec![Token::Real(1000.0)]);
        assert_eq!(toks(b"1E-3"), vec![Token::Real(0.001)]);
        // A second sign is junk: stop before it.
        assert_eq!(
            toks(b"1+-2"),
            vec![Token::Int(1), Token::Int(-2)]
        );
        // 1e999 overflows to infinity, which we must not produce.
        assert!(toks(b"1e999").is_empty() || toks(b"1e999") == vec![Token::Real(1.0)]);
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            toks(b"% hello\n42 % world\n 7"),
            vec![Token::Int(42), Token::Int(7)]
        );
        assert_eq!(toks(b"4%5"), vec![Token::Int(4), Token::Int(5)]);
    }

    #[test]
    fn keywords() {
        assert_eq!(toks(b"12 0 R obj endobj stream"), {
            vec![
                Token::Int(12),
                Token::Int(0),
                Token::Keyword(b"R".to_vec()),
                Token::Keyword(b"obj".to_vec()),
                Token::Keyword(b"endobj".to_vec()),
                Token::Keyword(b"stream".to_vec()),
            ]
        });
    }

    #[test]
    fn spans_are_exact() {
        let data = b"  /Name  12 ";
        let mut lx = Lexer::new(data);
        let a = lx.next_token().expect("token");
        assert_eq!((a.start, a.end), (2, 7));
        assert_eq!(a.token, Token::Name(b"Name".to_vec()));
        let b = lx.next_token().expect("token");
        assert_eq!((b.start, b.end), (9, 11));
        assert_eq!(b.token, Token::Int(12));
    }

    #[test]
    fn cr_only_line_endings() {
        assert_eq!(toks(b"1\r2"), vec![Token::Int(1), Token::Int(2)]);
    }
}
