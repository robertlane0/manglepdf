//! Name trees: the catalogue's `/Names`, and the legacy `/Dests` dictionary.
//!
//! Both mechanisms exist in real files — often both in the same one, with the same
//! names in each. Readers must merge them, with the name tree taking precedence.

use mangle_syntax::{Dict, Object, Ref};

use crate::error::{Error, Result};
use crate::{MAX_TREE_DEPTH, MAX_TREE_ENTRIES, Resolver, follow, kids_of};

/// One `/Names` or `/Dests` tree, flattened.
#[derive(Debug, Clone, Default)]
pub struct NameTree {
    entries: Vec<(Vec<u8>, Object)>,
}

impl NameTree {
    /// Walk a name tree node and everything under it.
    pub fn build(resolver: &dyn Resolver, node: Ref) -> Result<Self> {
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let root = resolver
            .resolve(node)
            .and_then(|o| o.as_dict().cloned())
            .ok_or_else(|| Error::Dangling("the name tree is missing".into()))?;
        walk(resolver, &root, &mut entries, &mut seen, 0)?;
        Ok(Self { entries })
    }

    /// Build from a tree node, or from a bare name array.
    pub fn build_from_node(resolver: &dyn Resolver, node: Object) -> Result<Self> {
        match &node {
            Object::Array(_) => {
                let mut entries = Vec::new();
                let mut pending: Option<Vec<u8>> = None;
                for item in node.as_array().unwrap_or_default() {
                    match item {
                        Object::String(name) => pending = Some(name.clone()),
                        other => {
                            if let Some(name) = pending.take()
                                && let Some(value) = follow(resolver, other.clone())
                            {
                                entries.push((name, value));
                            }
                        }
                    }
                }
                Ok(Self { entries })
            }
            _ => {
                let Some(d) = node.as_dict().cloned() else {
                    return Err(Error::Dangling("the name tree is not a dictionary".into()));
                };
                let mut entries = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                walk(resolver, &d, &mut entries, &mut seen, 0)?;
                Ok(Self { entries })
            }
        }
    }

    /// Build from a legacy `/Dests` dictionary, whose keys are names directly.
    #[must_use]
    pub fn from_legacy(resolver: &dyn Resolver, dict: &Dict) -> Self {
        let mut entries = Vec::new();
        for (name, value) in dict.iter() {
            let Some(value) = follow(resolver, value.clone()) else {
                continue;
            };
            entries.push((name.as_bytes().to_vec(), value));
        }
        Self { entries }
    }

    /// Every name and its destination, in tree order.
    #[must_use]
    pub fn entries(&self) -> &[(Vec<u8>, Object)] {
        &self.entries
    }

    /// Look a name up. Both mechanisms spell names as bytes, so a lookup compares
    /// bytes and never guesses an encoding.
    #[must_use]
    pub fn get(&self, name: &[u8]) -> Option<&Object> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Merge two trees, with `other` winning on a name collision. A file may carry
    /// both mechanisms, and the name tree is the one the specification prefers.
    #[must_use]
    pub fn merged(mut self, other: NameTree) -> Self {
        for (name, value) in other.entries {
            match self.entries.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => self.entries.push((name, value)),
            }
        }
        self
    }
}

fn walk(
    resolver: &dyn Resolver,
    node: &Dict,
    out: &mut Vec<(Vec<u8>, Object)>,
    seen: &mut std::collections::BTreeSet<Ref>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(Error::Limit {
            kind: "name tree depth",
            limit: MAX_TREE_DEPTH,
        });
    }
    if out.len() >= MAX_TREE_ENTRIES {
        return Err(Error::Limit {
            kind: "name tree entries",
            limit: MAX_TREE_ENTRIES,
        });
    }
    if let Some(r) = node.get("Self").and_then(Object::as_ref_id)
        && !seen.insert(r)
    {
        return Ok(());
    }

    if let Some(names) = node.get("Names").and_then(|n| n.as_array()) {
        // `Names` is a flat array of alternating key and value.
        let mut pending: Option<Vec<u8>> = None;
        for item in names {
            if out.len() >= MAX_TREE_ENTRIES {
                break;
            }
            match item {
                Object::String(name) => {
                    pending = Some(name.clone());
                }
                other => {
                    if let Some(name) = pending.take()
                        && let Some(value) = follow(resolver, other.clone())
                    {
                        out.push((name, value));
                    }
                }
            }
        }
    }

    for kid in kids_of(resolver, node) {
        if let Some(d) = kid.as_dict() {
            walk(resolver, d, out, seen, depth + 1)?;
        }
    }
    Ok(())
}
