//! `ToUnicode` CMap parsing (ISO 32000-1 §9.10.2).

use std::collections::BTreeMap;

use mangle_syntax::object::{Dict, Object};
use mangle_syntax::stream::decode_stream;

use crate::encoding::agl;

/// Maximum number of character mappings read from a single `ToUnicode` stream.
pub const MAX_TO_UNICODE_ENTRIES: usize = 65_536;

/// A parsed `/ToUnicode` CMap mapping character codes or CIDs to Unicode text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToUnicode {
    entries: BTreeMap<u32, String>,
}

impl ToUnicode {
    /// Read and parse the `/ToUnicode` stream on a font dictionary, if present.
    #[must_use]
    pub fn from_font_dict(
        dict: &Dict,
        resolve: &dyn Fn(&Object) -> Option<Object>,
    ) -> Option<Self> {
        let entry = dict.get("ToUnicode")?;
        let resolved = resolve(entry).unwrap_or_else(|| entry.clone());
        let Object::Stream(stream) = resolved else {
            return None;
        };
        let decoded = decode_stream(&stream);
        Some(Self::parse(&decoded.data))
    }

    /// Parse a `ToUnicode` CMap from raw (decoded) stream bytes.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        let tokens = tokenize(bytes);
        let mut entries = BTreeMap::new();
        let mut i = 0usize;
        while let Some(tok) = tokens.get(i) {
            match tok {
                CmapToken::Word(w) if w == "beginbfchar" => {
                    i += 1;
                    while let Some(item) = tokens.get(i) {
                        if matches!(item, CmapToken::Word(w) if w == "endbfchar") {
                            i += 1;
                            break;
                        }
                        let Some(src_bytes) = item.as_hex() else {
                            i += 1;
                            continue;
                        };
                        let Some(dst_tok) = tokens.get(i + 1) else {
                            break;
                        };
                        if let (Some(code), Some(text)) =
                            (code_from_bytes(src_bytes), target_string(dst_tok))
                            && !text.is_empty()
                            && entries.len() < MAX_TO_UNICODE_ENTRIES
                        {
                            entries.insert(code, text);
                        }
                        i += 2;
                    }
                }
                CmapToken::Word(w) if w == "beginbfrange" => {
                    i += 1;
                    while let Some(lo_tok) = tokens.get(i) {
                        if matches!(lo_tok, CmapToken::Word(w) if w == "endbfrange") {
                            i += 1;
                            break;
                        }
                        let Some(lo_bytes) = lo_tok.as_hex() else {
                            i += 1;
                            continue;
                        };
                        let Some(hi_bytes) = tokens.get(i + 1).and_then(CmapToken::as_hex) else {
                            break;
                        };
                        let Some(target) = tokens.get(i + 2) else {
                            break;
                        };
                        if let (Some(lo), Some(hi)) =
                            (code_from_bytes(lo_bytes), code_from_bytes(hi_bytes))
                            && lo <= hi
                        {
                            insert_range(&mut entries, lo, hi, target);
                        }
                        i += 3;
                    }
                }
                _ => {
                    i += 1;
                }
            }
            if entries.len() >= MAX_TO_UNICODE_ENTRIES {
                break;
            }
        }
        Self { entries }
    }

    /// Look up the Unicode text mapped from `code`.
    #[must_use]
    pub fn get(&self, code: u32) -> Option<&str> {
        self.entries.get(&code).map(String::as_str)
    }

    /// Find the code or CID that maps to the single character `ch`.
    ///
    /// Prefers the identity code point when it maps back to `ch`, otherwise returns the
    /// unique code that maps to `ch`.
    #[must_use]
    pub fn code_for(&self, ch: char) -> Option<u32> {
        let mut buf = [0u8; 4];
        let want = ch.encode_utf8(&mut buf);
        let id = u32::from(ch);
        if self.get(id) == Some(want) {
            return Some(id);
        }
        let mut found = None;
        for (&code, text) in &self.entries {
            if text == want {
                if found.is_some() {
                    return None;
                }
                found = Some(code);
            }
        }
        found
    }

    /// Number of mapped character codes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no mappings were found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn insert_range(entries: &mut BTreeMap<u32, String>, lo: u32, hi: u32, target: &CmapToken) {
    let cap = u32::try_from(MAX_TO_UNICODE_ENTRIES).unwrap_or(65_536);
    match target {
        CmapToken::Hex(base) => {
            for offset in 0..=hi.saturating_sub(lo).min(cap) {
                if entries.len() >= MAX_TO_UNICODE_ENTRIES {
                    break;
                }
                let Some(code) = lo.checked_add(offset) else {
                    break;
                };
                if let Some(text) = decode_utf16_offset(base, offset)
                    && !text.is_empty()
                {
                    entries.insert(code, text);
                }
            }
        }
        CmapToken::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                if entries.len() >= MAX_TO_UNICODE_ENTRIES {
                    break;
                }
                let Ok(offset) = u32::try_from(idx) else {
                    break;
                };
                let Some(code) = lo.checked_add(offset) else {
                    break;
                };
                if code > hi {
                    break;
                }
                if let Some(text) = target_string(item)
                    && !text.is_empty()
                {
                    entries.insert(code, text);
                }
            }
        }
        CmapToken::Name(_) | CmapToken::Word(_) => {}
    }
}

fn code_from_bytes(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut acc = 0u32;
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
    }
    Some(acc)
}

fn target_string(tok: &CmapToken) -> Option<String> {
    match tok {
        CmapToken::Hex(bytes) => decode_utf16_offset(bytes, 0),
        CmapToken::Name(name) => agl(name).and_then(char::from_u32).map(|c| c.to_string()),
        CmapToken::Word(_) | CmapToken::Array(_) => None,
    }
}

fn decode_utf16_offset(bytes: &[u8], offset: u32) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() == 1 {
        let base = u32::from(*bytes.first()?);
        let ch = char::from_u32(base.checked_add(offset)?)?;
        return Some(ch.to_string());
    }
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut units: Vec<u16> = bytes
        .chunks_exact(2)
        .filter_map(|pair| {
            let hi = *pair.first()?;
            let lo = *pair.get(1)?;
            Some(u16::from_be_bytes([hi, lo]))
        })
        .collect();
    let last = units.last_mut()?;
    let next = u32::from(*last).checked_add(offset)?;
    *last = u16::try_from(next).ok()?;
    let mut out = String::new();
    for decoded in char::decode_utf16(units) {
        out.push(decoded.ok()?);
    }
    Some(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CmapToken {
    Hex(Vec<u8>),
    Name(String),
    Word(String),
    Array(Vec<CmapToken>),
}

impl CmapToken {
    fn as_hex(&self) -> Option<&[u8]> {
        match self {
            Self::Hex(b) => Some(b),
            _ => None,
        }
    }
}

fn tokenize(bytes: &[u8]) -> Vec<CmapToken> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'%' {
            while let Some(&c) = bytes.get(i) {
                i += 1;
                if c == b'\n' || c == b'\r' {
                    break;
                }
            }
            continue;
        }
        if b == b'<' {
            if bytes.get(i + 1) == Some(&b'<') {
                i += 2;
                continue;
            }
            i += 1;
            let (hex, next) = read_hex(bytes, i);
            i = next;
            out.push(CmapToken::Hex(hex));
            continue;
        }
        if b == b'>' {
            i += 1;
            continue;
        }
        if b == b'[' {
            i += 1;
            let (items, next) = read_array(bytes, i);
            i = next;
            out.push(CmapToken::Array(items));
            continue;
        }
        if b == b']' {
            i += 1;
            continue;
        }
        if b == b'/' {
            i += 1;
            let start = i;
            while let Some(&c) = bytes.get(i) {
                if c.is_ascii_whitespace() || matches!(c, b'<' | b'>' | b'[' | b']' | b'/' | b'%') {
                    break;
                }
                i += 1;
            }
            if let Some(slice) = bytes.get(start..i) {
                out.push(CmapToken::Name(String::from_utf8_lossy(slice).into_owned()));
            }
            continue;
        }
        let start = i;
        while let Some(&c) = bytes.get(i) {
            if c.is_ascii_whitespace() || matches!(c, b'<' | b'>' | b'[' | b']' | b'/' | b'%') {
                break;
            }
            i += 1;
        }
        if let Some(slice) = bytes.get(start..i) {
            out.push(CmapToken::Word(String::from_utf8_lossy(slice).into_owned()));
        }
    }
    out
}

fn read_hex(bytes: &[u8], mut i: usize) -> (Vec<u8>, usize) {
    let mut digits = Vec::new();
    while let Some(&c) = bytes.get(i) {
        i += 1;
        if c == b'>' {
            break;
        }
        if let Some(nibble) = hex_nibble(c) {
            digits.push(nibble);
        }
    }
    if digits.len() % 2 == 1 {
        digits.push(0);
    }
    let out = digits
        .chunks_exact(2)
        .filter_map(|pair| Some((pair.first()? << 4) | pair.get(1)?))
        .collect();
    (out, i)
}

fn read_array(bytes: &[u8], mut i: usize) -> (Vec<CmapToken>, usize) {
    let mut items = Vec::new();
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b']' {
            i += 1;
            break;
        }
        if b == b'<' {
            i += 1;
            let (hex, next) = read_hex(bytes, i);
            i = next;
            items.push(CmapToken::Hex(hex));
            continue;
        }
        if b == b'/' {
            i += 1;
            let start = i;
            while let Some(&c) = bytes.get(i) {
                if c.is_ascii_whitespace() || matches!(c, b'<' | b'>' | b'[' | b']' | b'/' | b'%') {
                    break;
                }
                i += 1;
            }
            if let Some(slice) = bytes.get(start..i) {
                items.push(CmapToken::Name(String::from_utf8_lossy(slice).into_owned()));
            }
            continue;
        }
        i += 1;
    }
    (items, i)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::ToUnicode;

    #[test]
    fn bfchar_maps_single_and_multi_character_entries() {
        let cmap = br"
            /CIDInit /ProcSet findresource begin
            12 dict begin
            begincmap
            1 beginbfrange
            <0000> <0000> <FFFD>
            endbfrange
            3 beginbfchar
            <0001> <0045>
            <0002> <00660069>
            <0003> <D83DDE00>
            endbfchar
            endcmap
        ";
        let map = ToUnicode::parse(cmap);
        assert_eq!(map.get(1), Some("E"));
        assert_eq!(map.get(2), Some("fi"));
        assert_eq!(map.get(3), Some("😀"));
        assert_eq!(map.code_for('E'), Some(1));
        assert_eq!(map.code_for('😀'), Some(3));
        assert_eq!(map.code_for('z'), None);
    }

    #[test]
    fn bfrange_supports_both_contiguous_and_array_targets() {
        let cmap = br"
            2 beginbfrange
            <0010> <0012> <0041>
            <0020> <0022> [<0058> <0059> <0066006C>]
            endbfrange
        ";
        let map = ToUnicode::parse(cmap);
        assert_eq!(map.get(0x10), Some("A"));
        assert_eq!(map.get(0x11), Some("B"));
        assert_eq!(map.get(0x12), Some("C"));
        assert_eq!(map.get(0x20), Some("X"));
        assert_eq!(map.get(0x21), Some("Y"));
        assert_eq!(map.get(0x22), Some("fl"));
        assert_eq!(map.code_for('B'), Some(0x11));
        assert_eq!(map.code_for('Y'), Some(0x21));
    }

    #[test]
    fn huge_range_is_bounded() {
        let cmap = b"1 beginbfrange <00000000> <FFFFFFFF> <0020> endbfrange";
        let map = ToUnicode::parse(cmap);
        assert!(map.len() <= super::MAX_TO_UNICODE_ENTRIES);
    }
}
