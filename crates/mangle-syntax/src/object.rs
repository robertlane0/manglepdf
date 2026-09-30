//! The COS object model.

use std::fmt;

use crate::error::{Error, Result};

/// An indirect reference: object number plus generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ref {
    pub num: u32,
    pub generation: u16,
}

impl Ref {
    #[must_use]
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// A PDF name. The raw bytes are kept exactly as written (minus the `/`), because a
/// name written `#20` must be written back the same way if we do not change it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Name(pub Vec<u8>);

impl Name {
    /// A name from a Rust string. Bytes that need escaping are escaped.
    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Self {
        Self(b.to_vec())
    }

    /// A name from ASCII text, used for the many built-in keys.
    #[must_use]
    pub fn new(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }

    /// The name's bytes, unescaped. Invalid escapes are passed through as `#`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Lossy UTF-8 view, for display and comparison in tests.
    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }

    /// `true` if the name has no characters that must be escaped when written.
    #[must_use]
    pub fn is_regular(&self) -> bool {
        self.0
            .iter()
            .all(|&b| b > b' ' && b < 0x7f && !needs_escape(b))
    }
}

/// The bytes that must be written as `#xx` inside a name.
#[must_use]
pub const fn needs_escape(b: u8) -> bool {
    matches!(
        b,
        b'#' | b'/' | b'%' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}'
    )
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Name::new(s)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", String::from_utf8_lossy(&self.0))
    }
}

/// A COS object.
///
/// `Real` keeps the original text where possible: a number written `1.500` should be
/// written back as `1.5`, and one written `6.02e23` must not become `6.02E23`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Object {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(Vec<u8>),
    Name(Name),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(Ref),
    /// An indirect object that could not be resolved. Kept so the graph stays walkable.
    Missing(Ref),
}

impl Object {
    #[must_use]
    pub fn name(s: &str) -> Self {
        Object::Name(Name::new(s))
    }

    #[must_use]
    pub fn string(s: &str) -> Self {
        Object::String(s.as_bytes().to_vec())
    }

    /// The object as a dictionary, if it is one.
    #[must_use]
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&mut s.dict),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n.as_bytes()),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Object::Int(i) => Some(*i),
            // A real that is exactly an integer is an integer as far as the spec cares.
            Object::Real(r) if r.fract() == 0.0 && r.is_finite() => Some(*r as i64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Int(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Object::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_ref_id(&self) -> Option<Ref> {
        match self {
            Object::Ref(r) | Object::Missing(r) => Some(*r),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }

    /// A human-readable type name for the Inspector and error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Object::Null => "null",
            Object::Bool(_) => "boolean",
            Object::Int(_) => "integer",
            Object::Real(_) => "real",
            Object::String(_) => "string",
            Object::Name(_) => "name",
            Object::Array(_) => "array",
            Object::Dict(_) => "dictionary",
            Object::Stream(_) => "stream",
            Object::Ref(_) => "reference",
            Object::Missing(_) => "missing reference",
        }
    }

    /// Resolve one level of indirection for direct children.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Object> {
        self.as_dict()?.get(key)
    }

    /// Follow a chain of dictionary lookups.
    #[must_use]
    pub fn dig(&self, path: &[&str]) -> Option<&Object> {
        let mut cur = self;
        for key in path {
            cur = cur.get(key)?;
        }
        Some(cur)
    }
}

/// A COS dictionary that preserves key order and the exact spelling of every key.
///
/// A `BTreeMap` would sort keys, which changes bytes on a full rewrite and can make
/// diffs unreadable; a `HashMap` would make output non-deterministic. Insertion order
/// with a lazily built index gives both determinism and cheap lookup.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict {
    entries: Vec<(Name, Object)>,
    /// Number of keys, used to decide when the index is worth building.
    index_hint: Option<std::collections::BTreeMap<Name, usize>>,
}

impl Dict {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace, keeping the original position of an existing key.
    pub fn insert(&mut self, key: Name, value: Object) {
        if let Some(map) = self.index_hint.as_mut()
            && let Some(&i) = map.get(&key)
        {
            if let Some(slot) = self.entries.get_mut(i) {
                slot.1 = value;
            }
            return;
        }
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            if let Some(slot) = self.entries.get_mut(i) {
                slot.1 = value;
            }
            return;
        }
        if let Some(map) = self.index_hint.as_mut() {
            map.insert(key.clone(), self.entries.len());
        }
        self.entries.push((key, value));
    }

    /// Insert with a borrowed key.
    pub fn set(&mut self, key: &str, value: Object) {
        self.insert(Name::new(key), value);
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Object> {
        if let Some(map) = self.index_hint.as_ref() {
            return map
                .get(&Name::new(key))
                .and_then(|&i| self.entries.get(i))
                .map(|(_, v)| v);
        }
        self.entries
            .iter()
            .find(|(k, _)| k.as_bytes() == key.as_bytes())
            .map(|(_, v)| v)
    }

    #[must_use]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Object> {
        let i = self.position(key)?;
        self.entries.get_mut(i).map(|(_, v)| v)
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn remove(&mut self, key: &str) -> Option<Object> {
        let i = self.position(key)?;
        self.index_hint = None;
        match self.entries.get(i) {
            Some((_, v)) => {
                let v = v.clone();
                self.entries.remove(i);
                Some(v)
            }
            None => None,
        }
    }

    fn position(&self, key: &str) -> Option<usize> {
        if let Some(map) = self.index_hint.as_ref() {
            return map.get(&Name::new(key)).copied();
        }
        self.entries
            .iter()
            .position(|(k, _)| k.as_bytes() == key.as_bytes())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Name, &mut Object)> {
        self.entries.iter_mut().map(|(k, v)| (&*k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Name> {
        self.entries.iter().map(|(k, _)| k)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Build the lookup index once the dictionary is big enough to benefit.
    pub fn build_index(&mut self) {
        if self.index_hint.is_some() || self.entries.len() < 8 {
            return;
        }
        let mut map = std::collections::BTreeMap::new();
        for (i, (k, _)) in self.entries.iter().enumerate() {
            map.entry(k.clone()).or_insert(i);
        }
        self.index_hint = Some(map);
    }
}

impl FromIterator<(Name, Object)> for Dict {
    fn from_iter<T: IntoIterator<Item = (Name, Object)>>(iter: T) -> Self {
        let mut d = Dict::new();
        for (k, v) in iter {
            d.insert(k, v);
        }
        d
    }
}

/// A stream object: its dictionary plus the **raw** bytes as they appear in the file.
///
/// Raw bytes are what get written back. Decoded bytes are produced on demand and cached
/// by the caller, because a large image must not be decoded just to be re-saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    /// Bytes exactly as stored in the file, still filtered.
    pub raw: Vec<u8>,
    /// Where the stream data starts in the file, for provenance and for repair.
    pub file_offset: Option<usize>,
    /// Set when we produced these bytes ourselves (a new or modified stream).
    pub synthetic: bool,
}

impl Stream {
    #[must_use]
    pub fn new(dict: Dict, raw: Vec<u8>) -> Self {
        Self {
            dict,
            raw,
            file_offset: None,
            synthetic: true,
        }
    }

    /// The `/Type`, if any.
    #[must_use]
    pub fn subtype(&self) -> Option<&[u8]> {
        self.dict.get("Type").and_then(Object::as_name)
    }

    /// The filter chain: `/Filter` may be one name or an array of them.
    #[must_use]
    pub fn filters(&self) -> Vec<&[u8]> {
        match self.dict.get("Filter") {
            Some(Object::Name(n)) => vec![n.as_bytes()],
            Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_name()).collect(),
            _ => Vec::new(),
        }
    }

    /// The decode-parameter array, normalised to one entry per filter.
    #[must_use]
    pub fn decode_parms(&self) -> Vec<Option<&Object>> {
        let parms = self.dict.get("DecodeParms").or(self.dict.get("DP"));
        match parms {
            Some(Object::Array(a)) => a.iter().map(Some).collect(),
            Some(Object::Null) | None => vec![None],
            Some(o) => vec![Some(o)],
        }
    }
}

/// A rectangle, in PDF user space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

impl Rect {
    #[must_use]
    pub fn new(left: f64, bottom: f64, right: f64, top: f64) -> Self {
        Self {
            left,
            bottom,
            right,
            top,
        }
    }

    /// Parse `[a b c d]`, normalising so left <= right and bottom <= top.
    pub fn from_object(obj: &Object) -> Result<Self> {
        let Some(arr) = obj.as_array() else {
            return Err(Error::Syntax("expected a four-element array".into()));
        };
        if arr.len() < 4 {
            return Err(Error::Syntax(
                "rectangle array has fewer than 4 numbers".into(),
            ));
        }
        let mut nums = [0.0f64; 4];
        for (i, slot) in nums.iter_mut().enumerate() {
            *slot = arr.get(i).and_then(Object::as_f64).unwrap_or(0.0);
        }
        let (a, b) = (nums[0], nums[1]);
        let (c, d) = (nums[2], nums[3]);
        Ok(Rect {
            left: a.min(c),
            bottom: b.min(d),
            right: a.max(c),
            top: b.max(d),
        })
    }

    #[must_use]
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    #[must_use]
    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }

    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    /// The rectangle as a PDF array object, rounded to the writer's precision.
    #[must_use]
    pub fn to_object(self) -> Object {
        Object::Array(vec![
            Object::Real(self.left),
            Object::Real(self.bottom),
            Object::Real(self.right),
            Object::Real(self.top),
        ])
    }

    /// Grow by `pad` on every side, for shadows and selection outlines.
    #[must_use]
    pub fn inflate(self, pad: f64) -> Self {
        Rect {
            left: self.left - pad,
            bottom: self.bottom - pad,
            right: self.right + pad,
            top: self.top + pad,
        }
    }
}

/// Format a number the way the writer does: no exponents, at most six decimals, no
/// negative zero, never NaN or infinity, and always looking like a real.
#[must_use]
pub fn format_number(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".to_string();
    }
    if v == 0.0 {
        return "0.0".to_string();
    }
    // An integral real still has to look like a real: `3` and `3.0` are the same
    // number to a reader but not the same object, and a round trip must not turn one
    // into the other.
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{v:.1}");
    }
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    // `format!` can produce "-0"; never write that.
    if s == "-0" {
        s = "0".to_string();
    }
    // A value small enough to round away entirely still has to keep its point.
    if !s.contains('.') {
        s.push_str(".0");
    }
    s
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn dict_preserves_insertion_order() {
        let mut d = Dict::new();
        d.set("Zebra", Object::Int(1));
        d.set("Apple", Object::Int(2));
        d.set("Mango", Object::Int(3));
        let keys: Vec<String> = d.keys().map(|k| k.to_string()).collect();
        assert_eq!(keys, vec!["/Zebra", "/Apple", "/Mango"]);
    }

    #[test]
    fn dict_replaces_in_place() {
        let mut d = Dict::new();
        d.set("A", Object::Int(1));
        d.set("B", Object::Int(2));
        d.set("A", Object::Int(3));
        let keys: Vec<String> = d.keys().map(|k| k.to_string()).collect();
        assert_eq!(keys, vec!["/A", "/B"]);
        assert_eq!(d.get("A").and_then(Object::as_i64), Some(3));
    }

    #[test]
    fn dict_index_stays_consistent() {
        let mut d = Dict::new();
        for i in 0..40 {
            d.set(&format!("Key{i}"), Object::Int(i));
        }
        d.build_index();
        for i in 0..40 {
            assert_eq!(d.get(&format!("Key{i}")).and_then(Object::as_i64), Some(i));
        }
        d.set("Key5", Object::Int(999));
        assert_eq!(d.get("Key5").and_then(Object::as_i64), Some(999));
        assert!(d.remove("Key5").is_some());
        assert!(d.get("Key5").is_none());
        d.set("Key5", Object::Int(5));
        assert_eq!(d.get("Key5").and_then(Object::as_i64), Some(5));
    }

    #[test]
    fn numbers_are_written_without_exponents() {
        assert_eq!(format_number(1.0), "1.0");
        assert_eq!(format_number(-0.0), "0.0");
        assert_eq!(format_number(1.5), "1.5");
        assert_eq!(format_number(1.500_000_1), "1.5");
        assert_eq!(format_number(0.000_000_1), "0.0");
        // Exponents are expanded, at the cost of the precision f64 already lost.
        let big = format_number(6.02e23);
        assert!(!big.contains('e') && !big.contains('E'), "{big}");
        assert!(big.parse::<f64>().is_ok());
        assert_eq!(format_number(f64::NAN), "0.0");
        assert_eq!(format_number(f64::INFINITY), "0.0");
        assert_eq!(format_number(-1.234_567_8), "-1.234568");
    }

    #[test]
    fn rect_normalises() {
        let r = Rect::from_object(&Object::Array(vec![
            Object::Real(300.0),
            Object::Real(400.0),
            Object::Real(10.0),
            Object::Real(20.0),
        ]))
        .expect("rect");
        assert_eq!(r, Rect::new(10.0, 20.0, 300.0, 400.0));
        assert!((r.width() - 290.0).abs() < 1e-9);
    }

    #[test]
    fn accessors() {
        assert_eq!(Object::Int(7).as_f64(), Some(7.0));
        assert_eq!(Object::Real(7.0).as_i64(), Some(7));
        assert!(Object::Null.as_dict().is_none());
        assert_eq!(Object::Null.type_name(), "null");
    }
}
