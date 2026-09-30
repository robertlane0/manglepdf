//! The page tree: flattening it, and resolving the attributes a page inherits.
//!
//! Two things make this harder than it looks. A page may omit an inheritable attribute
//! and rely on an ancestor, and the tree may be a `Pages` node, a `Page` leaf, a
//! mixture of both with missing `/Type` keys, or a cycle.

use mangle_syntax::{Dict, Object, Rect, Ref, Stream};

use crate::error::{Error, Result};
use crate::{MAX_TREE_DEPTH, MAX_TREE_ENTRIES, Resolver, follow, is_page};

/// The attributes a page inherits from its ancestors.
///
/// ISO 32000-1 Table 30. Nothing else is inheritable, and inheriting anything else
/// silently changes how a page renders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inheritable {
    pub resources: Option<Object>,
    pub media_box: Option<Object>,
    pub crop_box: Option<Object>,
    pub rotate: Option<Object>,
}

impl Inheritable {
    /// Take from a node dictionary the attributes it actually sets.
    fn absorb(&mut self, d: &Dict) {
        for (key, slot) in [
            ("Resources", &mut self.resources),
            ("MediaBox", &mut self.media_box),
            ("CropBox", &mut self.crop_box),
            ("Rotate", &mut self.rotate),
        ] {
            if let Some(v) = d.get(key) {
                *slot = Some(v.clone());
            }
        }
    }

    /// The media box, normalised. Falls back to US Letter, which is what the
    /// specification says to assume.
    #[must_use]
    pub fn media_rect(&self) -> Rect {
        self.media_box
            .as_ref()
            .and_then(|o| Rect::from_object(o).ok())
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 612.0, 792.0))
    }

    /// The crop box, normalised, and intersected with the media box. A crop box
    /// outside the media box is clamped rather than rejected: the file is wrong, but
    /// showing nothing is worse.
    #[must_use]
    pub fn crop_rect(&self) -> Rect {
        let media = self.media_rect();
        let Some(crop) = self
            .crop_box
            .as_ref()
            .and_then(|o| Rect::from_object(o).ok())
        else {
            return media;
        };
        Rect::new(
            crop.left.max(media.left),
            crop.bottom.max(media.bottom),
            crop.right.min(media.right),
            crop.top.min(media.top),
        )
    }

    /// `/Rotate`, rounded to a right angle and normalised to 0-270.
    #[must_use]
    pub fn rotation(&self) -> i32 {
        self.rotate
            .as_ref()
            .and_then(Object::as_i64)
            .map_or(0, crate::normalise_rotate)
    }

    /// Width and height after `/Rotate` is applied, which is what a viewer must lay
    /// the page out at.
    #[must_use]
    pub fn displayed_size(&self) -> (f64, f64) {
        let r = self.crop_rect();
        let (w, h) = (r.width(), r.height());
        if self.rotation() % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// One page, resolved.
#[derive(Debug, Clone)]
pub struct Page {
    /// The page dictionary.
    pub dict: Dict,
    /// Its position in document order, from zero.
    pub index: usize,
    /// Attributes resolved through the tree.
    pub inherited: Inheritable,
}

impl Page {
    /// The object number, when the page came from one.
    #[must_use]
    pub fn object_ref(&self) -> Option<Ref> {
        None
    }

    /// The page's content streams, concatenated with a newline between them, exactly
    /// as the specification says to treat `/Contents`. The bytes are still encoded.
    #[must_use]
    pub fn contents(&self, resolver: &dyn Resolver) -> Vec<u8> {
        let Some(contents) = self.dict.get("Contents") else {
            return Vec::new();
        };
        match contents {
            Object::Ref(r) => match resolver.resolve(*r) {
                Some(Object::Stream(s)) => s.raw,
                _ => Vec::new(),
            },
            Object::Stream(s) => s.raw.clone(),
            Object::Array(parts) => {
                let mut out = Vec::new();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        out.push(b'\n');
                    }
                    let stream = match part {
                        Object::Ref(r) => resolver.resolve(*r),
                        other => Some(other.clone()),
                    };
                    if let Some(Object::Stream(s)) = stream {
                        out.extend_from_slice(&s.raw);
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// The page's content streams with their filters applied.
    ///
    /// `/Contents` may be a stream, an array of streams, or references to either, and a
    /// filter is as likely to be in the way as not. A caller that wants the text of a
    /// page should not have to know that.
    #[must_use]
    pub fn decoded_contents(&self, resolver: &dyn Resolver) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, part) in self.content_streams(resolver).into_iter().enumerate() {
            if i > 0 {
                out.push(b'\n');
            }
            out.extend_from_slice(&resolver.decoded(&part));
        }
        out
    }

    /// The page's content streams, decoded one by one.
    #[must_use]
    pub fn content_streams(&self, resolver: &dyn Resolver) -> Vec<Stream> {
        let Some(contents) = self.dict.get("Contents") else {
            return Vec::new();
        };
        let parts: Vec<Object> = match contents {
            Object::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
        parts
            .iter()
            .filter_map(|p| match p {
                Object::Ref(r) => resolver.resolve(*r),
                other => Some(other.clone()),
            })
            .filter_map(|o| match o {
                Object::Stream(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    /// The annotations, in the order they are drawn.
    #[must_use]
    pub fn annots(&self, resolver: &dyn Resolver) -> Vec<Object> {
        self.dict
            .get("Annots")
            .and_then(|a| follow(resolver, a.clone()))
            .and_then(|a| a.as_array().map(<[Object]>::to_vec))
            .unwrap_or_default()
    }

    /// The object number of a page that came from the file.
    #[must_use]
    pub fn number(&self) -> u32 {
        self.dict
            .get("Self")
            .and_then(Object::as_ref_id)
            .map_or(0, |r| r.num)
    }
}

/// The flattened page tree.
#[derive(Debug, Clone, Default)]
pub struct PageTree {
    pages: Vec<Page>,
}

impl PageTree {
    /// Walk the tree from the catalogue's `/Pages`, resolving inheritance as it goes.
    pub fn build(resolver: &dyn Resolver, root: Ref) -> Result<Self> {
        let mut pages = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let start = resolver
            .resolve(root)
            .and_then(|o| o.as_dict().cloned())
            .ok_or_else(|| Error::Dangling("the page tree root is missing".into()))?;
        walk(
            resolver,
            root,
            &start,
            &Inheritable::default(),
            &mut pages,
            &mut seen,
            0,
        )?;
        Ok(Self { pages })
    }

    /// The pages, in document order.
    #[must_use]
    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// The page at a position.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Page> {
        self.pages.get(index)
    }

    /// What `/Count` claims, next to what we actually found. A mismatch is a finding,
    /// not a reason to stop: a file with a wrong count is still readable.
    #[must_use]
    pub fn claimed_count(&self, resolver: &dyn Resolver, root: Ref) -> Option<i64> {
        resolver
            .resolve(root)
            .and_then(|o| o.get("Count").and_then(Object::as_i64))
    }
}

/// The object numbers a tree node's `/Kids` names, without resolving them.
///
/// The references are what a cycle is made of, so they have to survive until the walk
/// can compare them. A `/Kids` array may also hold dictionaries directly, which is
/// damage but not fatal: those are skipped here and the pages around them are kept.
#[must_use]
pub fn kid_refs_of(resolver: &dyn Resolver, node: &Dict) -> Vec<Ref> {
    node.get("Kids")
        .and_then(|k| follow(resolver, k.clone()))
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
        .map(|kids| kids.iter().filter_map(Object::as_ref_id).collect())
        .unwrap_or_default()
}

/// Walk a node, tracking which object numbers have been entered.
///
/// The reference is carried alongside the dictionary because a dictionary has no
/// identity of its own, and a cycle is a property of the references. A `/Self` key
/// would do, but the specification has no such key: only a test that wrote one would
/// carry it.
fn walk(
    resolver: &dyn Resolver,
    id: Ref,
    node: &Dict,
    inherited: &Inheritable,
    out: &mut Vec<Page>,
    seen: &mut std::collections::BTreeSet<Ref>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(Error::Limit {
            kind: "page tree depth",
            limit: MAX_TREE_DEPTH,
        });
    }
    if out.len() >= MAX_TREE_ENTRIES {
        return Err(Error::Limit {
            kind: "page count",
            limit: MAX_TREE_ENTRIES,
        });
    }
    // A node may appear twice, but only once on the walk: a shared subtree is legal to
    // skip, and a cycle must not be followed. Both are handled the same way, because the
    // only thing that differs is why the second visit happened and neither is a reason
    // to lose the pages that are there.
    if !seen.insert(id) {
        return Ok(());
    }

    let mut here = inherited.clone();
    here.absorb(node);

    if is_page(node) {
        out.push(Page {
            dict: node.clone(),
            index: out.len(),
            inherited: here,
        });
        return Ok(());
    }

    for kid in kid_refs_of(resolver, node) {
        // The reference is what identifies the node; the dictionary is what describes
        // it. A `/Kids` entry that is neither is damage, and skipping it keeps the pages
        // around it.
        let Some(dict) = resolver.resolve(kid).and_then(|o| o.as_dict().cloned()) else {
            continue;
        };
        walk(resolver, kid, &dict, &here, out, seen, depth + 1)?;
    }
    Ok(())
}
