//! Embedded files: the `/EmbeddedFiles` name tree, and the file specifications that
//! describe them.

use mangle_syntax::{Dict, Object, Ref, Stream};

use crate::error::Result;
use crate::{MAX_TREE_DEPTH, Resolver, follow, kids_of};

/// A `/Filespec`: what the attachments panel lists.
#[derive(Debug, Clone)]
pub struct FileSpec {
    /// The spec's object number.
    pub id: Ref,
    /// `/UF`, `/F`, `/DOS`, `/Mac` or `/Unix`, in the order the specification prefers
    /// them. When none is present the name the name tree used is the name.
    pub name: String,
    /// `/Desc`.
    pub description: Option<String>,
    /// The legacy inline stream, where the bytes were placed directly in `/F`.
    pub inline: Option<Stream>,
    /// The `/EF` dictionary, resolved on demand because it is usually a reference and
    /// resolving it costs a parse.
    pub ef: Option<Object>,
}

impl FileSpec {
    /// The embedded bytes, preferring the platform entry that fits.
    ///
    /// The platform keys are tried in the order the specification defines, and the
    /// first that exists wins. Falling back to "any entry" is what makes a file that
    /// only has `/Unix` still work on every platform.
    #[must_use]
    pub fn bytes(&self, resolver: &dyn Resolver) -> Option<Vec<u8>> {
        if let Some(s) = &self.inline {
            return Some(s.raw.clone());
        }
        self.embedded(resolver).map(|s| s.raw)
    }

    /// The `/EF` stream for this file, or the inline one.
    #[must_use]
    pub fn embedded(&self, resolver: &dyn Resolver) -> Option<Stream> {
        let table = self.ef.as_ref().and_then(|e| follow(resolver, e.clone()))?;
        let table = table.as_dict()?;
        for want in ["UF", "F", "DOS", "Mac", "Unix"] {
            if let Some(Object::Stream(s)) =
                table.get(want).and_then(|o| follow(resolver, o.clone()))
            {
                return Some(s.clone());
            }
        }
        None
    }
}

/// The `/EmbeddedFiles` name tree.
#[derive(Debug, Clone, Default)]
pub struct EmbeddedFiles {
    pub files: Vec<FileSpec>,
}

impl EmbeddedFiles {
    /// Read the catalogue's `/Names` `/EmbeddedFiles` tree.
    pub fn build(resolver: &dyn Resolver, catalog: &Dict) -> Result<Self> {
        let Some(names) = catalog
            .get("Names")
            .and_then(|o| follow(resolver, o.clone()))
            .and_then(|o| o.as_dict().cloned())
        else {
            return Ok(Self::default());
        };
        let Some(root) = names
            .get("EmbeddedFiles")
            .and_then(|o| follow(resolver, o.clone()))
            .and_then(|o| o.as_dict().cloned())
        else {
            return Ok(Self::default());
        };

        let mut files = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        walk(resolver, &root, &mut files, &mut seen, 0);
        Ok(Self { files })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn walk(
    resolver: &dyn Resolver,
    node: &Dict,
    out: &mut Vec<FileSpec>,
    seen: &mut std::collections::BTreeSet<Ref>,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH || out.len() >= 100_000 {
        return;
    }
    if let Some(r) = node.get("Self").and_then(Object::as_ref_id)
        && !seen.insert(r)
    {
        return;
    }
    if let Some(names) = node.get("Names").and_then(|n| n.as_array()) {
        let mut pending: Option<Vec<u8>> = None;
        for item in names {
            match item {
                Object::String(name) => pending = Some(name.clone()),
                other => {
                    let Some(name) = pending.take() else { continue };
                    let Some(r) = other.as_ref_id() else { continue };
                    if !seen.insert(r) {
                        continue;
                    }
                    let Some(d) = resolver.resolve(r).and_then(|o| o.as_dict().cloned()) else {
                        continue;
                    };
                    out.push(read_spec(r, &name, &d));
                }
            }
        }
    }
    for kid in kids_of(resolver, node) {
        if let Some(d) = kid.as_dict() {
            walk(resolver, d, out, seen, depth + 1);
        }
    }
}

fn read_spec(id: Ref, tree_name: &[u8], d: &Dict) -> FileSpec {
    let name = ["UF", "F", "DOS", "Mac", "Unix"]
        .iter()
        .find_map(|k| d.get(k).and_then(Object::as_bytes))
        .map(crate::outlines::decode_pdf_text)
        .unwrap_or_else(|| crate::outlines::decode_pdf_text(tree_name));

    FileSpec {
        id,
        name,
        description: d
            .get("Desc")
            .and_then(Object::as_bytes)
            .map(crate::outlines::decode_pdf_text),
        inline: match d.get("F") {
            Some(Object::Stream(s)) => Some(s.clone()),
            _ => None,
        },
        ef: d.get("EF").cloned(),
    }
}
