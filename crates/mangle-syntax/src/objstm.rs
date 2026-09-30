//! Object streams (`/Type /ObjStm`).

use crate::error::{Error, Result};
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, Object, Ref, Stream};
use crate::stream::decode_stream;

/// A parsed object stream: the objects it contains and where each one starts.
#[derive(Debug, Clone, Default)]
pub struct ObjectStream {
    /// Object number to `(offset within the decoded data, generation)`.
    pub entries: Vec<(u32, (usize, u16))>,
    /// The decoded body.
    pub data: Vec<u8>,
    /// `/N` as written, for cross-checking.
    pub declared: usize,
    /// `/First` as written.
    pub first: usize,
}

impl ObjectStream {
    /// Parse an object stream. The stream's data must already be decoded.
    #[must_use]
    pub fn parse(stream: &Stream, decoded: &[u8]) -> Self {
        let n = stream
            .dict
            .get("N")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0) as usize;
        let first = stream
            .dict
            .get("First")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0) as usize;
        let mut out = Self {
            entries: Vec::new(),
            data: decoded.to_vec(),
            declared: n,
            first,
        };
        // The header is `objnum offset objnum offset ...` for `n` objects.
        let head = decoded.get(..first).unwrap_or(&[]);
        let mut lx = Lexer::new(head);
        let mut pairs = Vec::new();
        while let (Ok(a), Ok(b)) = (lx.next_token(), lx.next_token()) {
            match (a.token, b.token) {
                (Token::Int(num), Token::Int(off)) => pairs.push((num, off)),
                _ => break,
            }
        }
        for (i, (num, off)) in pairs.iter().enumerate() {
            let start = first.saturating_add(usize::try_from(*off).unwrap_or(0));
            let end = pairs
                .get(i + 1)
                .map(|(_, o)| first + usize::try_from(*o).unwrap_or(0))
                .unwrap_or(decoded.len());
            let _ = end;
            out.entries.push((
                u32::try_from(*num).unwrap_or(0),
                (start, u16::try_from(*num).unwrap_or(0)),
            ));
        }
        out
    }

    /// Build from a stream, decoding it first.
    #[must_use]
    pub fn from_stream(stream: &Stream) -> Self {
        let decoded = decode_stream(stream).data;
        Self::parse(stream, &decoded)
    }

    /// The object at `index`, in the order the stream lists them.
    #[must_use]
    pub fn object_at(&self, index: u32) -> Option<Object> {
        let (_, (start, _)) = *self.entries.get(index as usize)?;
        // The last object runs to the end of the body.
        let end = self
            .entries
            .get(index as usize + 1)
            .map_or(self.data.len(), |(_, (next, _))| *next);
        crate::parser::Parser::new(self.data.get(start..end)?)
            .next_object()
            .ok()
            .flatten()
    }

    /// The object with number `num`.
    #[must_use]
    pub fn object(&self, num: u32) -> Option<Object> {
        let index = self.entries.iter().position(|(n, _)| *n == num)?;
        self.object_at(u32::try_from(index).unwrap_or(0))
    }
}

/// Build a new object stream from a set of objects, returning `(header, body)`.
#[must_use]
pub fn build(objects: &[(u32, &Object)]) -> (Vec<u8>, Vec<u8>) {
    let mut header = Vec::new();
    let mut body = Vec::new();
    for (num, obj) in objects {
        header.extend_from_slice(num.to_string().as_bytes());
        header.push(b' ');
        header.extend_from_slice(body.len().to_string().as_bytes());
        header.push(b' ');
        write_object(&mut body, None, obj);
        body.push(b'\n');
    }
    (header, body)
}

fn write_object(out: &mut Vec<u8>, num: Option<u32>, obj: &Object) {
    if let Some(n) = num {
        out.extend_from_slice(n.to_string().as_bytes());
        out.extend_from_slice(b" 0 obj\n");
    }
    crate::writer::write_object_into(out, obj);
    if num.is_some() {
        out.extend_from_slice(b"\nendobj\n");
    }
}

/// The stream dictionary for a set of objects.
#[must_use]
pub fn stream_dict(n: usize, first: usize) -> Dict {
    let mut d = Dict::new();
    d.set("Type", Object::name("ObjStm"));
    d.set("N", Object::Int(n as i64));
    d.set("First", Object::Int(first as i64));
    d
}

/// Extract one object from an object stream, for the document loader.
pub fn extract(stream: &Stream, index: u32) -> Result<Object> {
    let os = ObjectStream::from_stream(stream);
    os.object_at(index).ok_or(Error::Structure(format!(
        "object stream has no entry {index}"
    )))
}

/// The reference for an object inside a stream.
#[must_use]
pub fn object_ref(num: u32, generation: u16) -> Ref {
    Ref::new(num, generation)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::object::Name;

    #[test]
    fn round_trip_through_an_object_stream() {
        let objects: Vec<(u32, Object)> = vec![
            (
                1,
                Object::Dict([(Name::new("A"), Object::Int(1))].into_iter().collect()),
            ),
            (2, Object::Array(vec![Object::Int(1), Object::Int(2)])),
            (3, Object::name("Hello")),
        ];
        let refs: Vec<(u32, &Object)> = objects.iter().map(|(n, o)| (*n, o)).collect();
        let (header, body) = build(&refs);
        let mut data = header.clone();
        data.extend_from_slice(&body);
        let d = stream_dict(objects.len(), header.len());
        let s = Stream::new(d, data);
        let os = ObjectStream::from_stream(&s);
        assert_eq!(os.entries.len(), 3);
        assert_eq!(
            os.object(1),
            Some(Object::Dict(
                [(Name::new("A"), Object::Int(1))].into_iter().collect()
            ))
        );
        assert_eq!(
            os.object(2),
            Some(Object::Array(vec![Object::Int(1), Object::Int(2)]))
        );
        assert_eq!(os.object(3), Some(Object::name("Hello")));
        assert_eq!(os.object(99), None);
    }

    #[test]
    fn offsets_are_preserved() {
        let objects: Vec<(u32, Object)> = vec![(7, Object::Int(1)), (8, Object::Int(2))];
        let refs: Vec<(u32, &Object)> = objects.iter().map(|(n, o)| (*n, o)).collect();
        let (header, body) = build(&refs);
        let mut data = header.clone();
        data.extend_from_slice(&body);
        let d = stream_dict(2, header.len());
        let os = ObjectStream::from_stream(&Stream::new(d, data));
        let first = os.entries.first().map(|(_, (at, _))| *at);
        let second = os.entries.get(1).map(|(_, (at, _))| *at);
        assert_eq!(first, Some(header.len()));
        assert!(
            second > first,
            "offsets must increase: {second:?} vs {first:?}"
        );
    }
}
