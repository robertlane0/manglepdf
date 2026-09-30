//! Bookmarks: the outline tree, and resolving an outline item to a place on a page.

use mangle_syntax::{Dict, Object, Ref};

use crate::error::Result;
use crate::{MAX_TREE_DEPTH, Resolver, follow};

/// A destination: a page and a position within it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Destination {
    /// Zero-based page index.
    pub page: usize,
    /// Left, bottom, right, top in a left-to-right coordinate space. `None` when the
    /// destination named no rectangle, which means "the page as it is".
    pub rect: Option<[f64; 4]>,
    /// `/XYZ` nulls: `None` keeps the current value rather than resetting it.
    pub xyz: Option<(Option<f64>, Option<f64>, Option<f64>)>,
    /// `/Fit`-style destinations carry no position.
    pub kind: Fit,
}

/// How a destination frames the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// `/XYZ left top zoom`.
    #[default]
    Xyz,
    Fit,
    FitH,
    FitV,
    FitR,
    FitB,
    /// `/FitB` is deprecated and means the same as `/Fit`.
    FitBp,
}

/// One bookmark.
#[derive(Debug, Clone)]
pub struct OutlineItem {
    pub title: Vec<u8>,
    /// A `/Dest` string or array, or a `/A` action's destination.
    pub destination: Option<Object>,
    pub children: Vec<OutlineItem>,
}

impl OutlineItem {
    /// The title as text, when it decodes cleanly. Bookmark titles are not required to
    /// be any particular encoding, so invalid bytes are replaced rather than dropped.
    #[must_use]
    pub fn title_text(&self) -> String {
        decode_pdf_text(&self.title)
    }
}

/// The whole outline tree.
#[derive(Debug, Clone, Default)]
pub struct Outline {
    pub items: Vec<OutlineItem>,
}

impl Outline {
    /// Read the catalogue's `/Outlines`, including `/First`, `/Last` and `/Count`.
    pub fn build(resolver: &dyn Resolver, root: Ref) -> Result<Self> {
        let mut items = Vec::new();
        let Some(node) = resolver.resolve(root).and_then(|o| o.as_dict().cloned()) else {
            return Ok(Self::default());
        };
        let mut seen = std::collections::BTreeSet::new();
        // `/First` and `/Last` form a linked list; following the list is how a
        // correctly ordered outline is read.
        let mut cursor = node.get("First").cloned();
        while let Some(item) = cursor {
            if items.len() >= MAX_ITEMS {
                break;
            }
            let Some(r) = item.as_ref_id() else { break };
            if !seen.insert(r) {
                break;
            }
            let Some(d) = resolver.resolve(r).and_then(|o| o.as_dict().cloned()) else {
                break;
            };
            cursor = d.get("Next").cloned();
            items.push(read_item(resolver, &d, &mut seen, 0));
        }
        Ok(Self { items })
    }

    /// Every item, flattened, parents before children.
    #[must_use]
    pub fn flatten(&self) -> Vec<&OutlineItem> {
        let mut out = Vec::new();
        for item in &self.items {
            flatten_into(item, &mut out);
        }
        out
    }
}

const MAX_ITEMS: usize = 200_000;

fn flatten_into<'a>(item: &'a OutlineItem, out: &mut Vec<&'a OutlineItem>) {
    out.push(item);
    for child in &item.children {
        flatten_into(child, out);
    }
}

fn read_item(
    resolver: &dyn Resolver,
    d: &Dict,
    seen: &mut std::collections::BTreeSet<Ref>,
    depth: usize,
) -> OutlineItem {
    let mut children = Vec::new();
    if depth < MAX_TREE_DEPTH {
        let mut cursor = d.get("First").cloned();
        while let Some(item) = cursor {
            if children.len() >= MAX_ITEMS {
                break;
            }
            let Some(r) = item.as_ref_id() else { break };
            if !seen.insert(r) {
                break;
            }
            let Some(kid) = resolver.resolve(r).and_then(|o| o.as_dict().cloned()) else {
                break;
            };
            cursor = kid.get("Next").cloned();
            children.push(read_item(resolver, &kid, seen, depth + 1));
        }
    }
    OutlineItem {
        title: d
            .get("Title")
            .and_then(Object::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default(),
        destination: d
            .get("Dest")
            .cloned()
            .or_else(|| action_destination(resolver, d)),
        children,
    }
}

/// A `/GoTo` action's `/D`, which is where a bookmark built by a link instead of a
/// `/Dest` puts its target.
fn action_destination(resolver: &dyn Resolver, d: &Dict) -> Option<Object> {
    let action = follow(resolver, d.get("A")?.clone())?;
    let action = action.as_dict()?;
    if action.get("S").and_then(Object::as_name) != Some(&b"GoTo"[..]) {
        return None;
    }
    action.get("D").cloned()
}

/// Turn a `/Dest` value into a destination, given a way to reach the page index.
///
/// `page_index` maps a page dictionary's object number to its position; a reference
/// that is not in the document resolves to page 0 rather than failing, because losing
/// the rest of a bookmark tree over one bad link is the wrong trade.
#[must_use]
pub fn resolve_destination(
    dest: &Object,
    page_index: &dyn Fn(Ref) -> Option<usize>,
) -> Option<Destination> {
    let arr = dest.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let page = match arr.first()? {
        Object::Ref(r) => page_index(*r).unwrap_or(0),
        _ => page_index(Ref::new(0, 0)).unwrap_or(0),
    };
    let spec = arr.get(1)?;
    let kind_name = spec.as_name()?;
    // The arguments are positional and a `null` holds its place: `/XYZ null 400 null`
    // means "keep the left edge, set the top". Dropping the nulls would silently turn
    // it into a left value, so each slot is read by index.
    let arg = |i: usize| arr.get(i + 2).and_then(Object::as_f64);
    let (kind, rect, xyz) = match kind_name {
        b"Fit" => (Fit::Fit, None, None),
        b"FitH" => (Fit::FitH, None, None),
        b"FitV" => (Fit::FitV, None, None),
        b"FitR" => (Fit::FitR, Some([arg(0)?, arg(1)?, arg(2)?, arg(3)?]), None),
        b"FitB" => (Fit::FitB, None, None),
        b"FitBP" => (Fit::FitBp, None, None),
        // Anything else is treated as `/XYZ`, which is what a reader must assume.
        _ => (Fit::Xyz, None, Some((arg(0), arg(1), arg(2)))),
    };
    Some(Destination {
        page,
        rect,
        xyz,
        kind,
    })
}

/// Decode a text string that may be PDFDocEncoded, UTF-16BE with a byte-order mark,
/// or UTF-8.
#[must_use]
pub fn decode_pdf_text(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], big_endian: bool| {
        // `chunks_exact` drops a trailing odd byte, which is what a truncated
        // UTF-16 string should do rather than invent a character from one byte.
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .filter_map(|c| match (c.first(), c.get(1)) {
                (Some(&a), Some(&b)) => Some(if big_endian {
                    u16::from_be_bytes([a, b])
                } else {
                    u16::from_le_bytes([a, b])
                }),
                _ => None,
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    // The byte-order mark is part of the string, not text.
    match bytes {
        [0xFE, 0xFF, rest @ ..] => return utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => return utf16(rest, false),
        _ => {}
    }
    bytes.iter().map(|b| pdfdoc_char(*b)).collect()
}

/// The StandardEncoding/WinAnsi range of PDFDocEncoding, which is close enough to
/// Latin-1 that the difference does not matter for a bookmark title.
fn pdfdoc_char(b: u8) -> char {
    match b {
        0x18..=0x1f | 0x80..=0x9f => char::from(b),
        _ => char::from(b),
    }
}
