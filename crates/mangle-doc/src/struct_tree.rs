//! Tagged PDF structure-tree traversal (`/StructTreeRoot`).
//!
//! Walks the structure tree in pre-order to recover the logical reading order of
//! marked-content identifiers (`MCID`s) for a given page, bounded by [`MAX_TREE_DEPTH`]
//! and [`MAX_TREE_ENTRIES`] with cycle detection on indirect references.

use std::collections::BTreeSet;

use mangle_syntax::{Dict, Object, Ref};

use crate::{MAX_TREE_DEPTH, MAX_TREE_ENTRIES, Resolver, follow_from};

/// Walk `/StructTreeRoot` in the catalogue and return the ordered `MCID`s for `page_id`.
///
/// Handles integer `/K` children, marked-content reference dictionaries (`/Type /MCR` with
/// `/MCID` and optional `/Pg`), nested structure elements (`/S`, `/Pg`, `/K`), and arrays
/// of children. When a structure element omits `/Pg`, its `MCID`s match `page_id` if no
/// enclosing element named a different page.
#[must_use]
pub fn page_reading_mcids(resolver: &dyn Resolver, catalog: &Dict, page_id: Ref) -> Vec<i64> {
    let Some(root_obj) = follow_from(resolver, catalog, "StructTreeRoot") else {
        return Vec::new();
    };
    let Some(root_dict) = root_obj.as_dict() else {
        return Vec::new();
    };
    let Some(kids) = root_dict.get("K") else {
        return Vec::new();
    };
    let mut walk = StructWalk {
        resolver,
        target_page: page_id,
        visited: BTreeSet::new(),
        steps: 0,
        out: Vec::new(),
    };
    walk.visit(kids, None, 0);
    walk.out
}

struct StructWalk<'a> {
    resolver: &'a dyn Resolver,
    target_page: Ref,
    visited: BTreeSet<Ref>,
    steps: usize,
    out: Vec<i64>,
}

impl StructWalk<'_> {
    fn visit(&mut self, node: &Object, current_page: Option<Ref>, depth: usize) {
        if depth > MAX_TREE_DEPTH || self.steps >= MAX_TREE_ENTRIES {
            return;
        }
        self.steps += 1;

        if let Some(r) = node.as_ref_id() {
            if !self.visited.insert(r) {
                return;
            }
            if let Some(resolved) = self.resolver.resolve(r) {
                self.visit(&resolved, current_page, depth + 1);
            }
            return;
        }

        match node {
            Object::Int(mcid) => {
                if current_page.is_none_or(|pg| pg == self.target_page) {
                    self.out.push(*mcid);
                }
            }
            Object::Array(items) => {
                for item in items {
                    self.visit(item, current_page, depth + 1);
                }
            }
            Object::Dict(dict) => {
                let page = dict.get("Pg").and_then(Object::as_ref_id).or(current_page);
                if let Some(mcid) = dict.get("MCID").and_then(Object::as_i64)
                    && page.is_none_or(|pg| pg == self.target_page)
                {
                    self.out.push(mcid);
                }
                if let Some(kids) = dict.get("K") {
                    self.visit(kids, page, depth + 1);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::MapResolver;

    fn dict(entries: &[(&str, Object)]) -> Object {
        let mut d = Dict::new();
        for (k, v) in entries {
            d.set(k, v.clone());
        }
        Object::Dict(d)
    }

    #[test]
    fn walks_struct_tree_in_reading_order_for_target_page() {
        let page1 = Ref::new(5, 0);
        let page2 = Ref::new(6, 0);
        let resolver = MapResolver::new()
            .with(
                10,
                dict(&[
                    ("Type", Object::name("StructTreeRoot")),
                    (
                        "K",
                        Object::Array(vec![
                            Object::Ref(Ref::new(11, 0)),
                            Object::Ref(Ref::new(12, 0)),
                        ]),
                    ),
                ]),
            )
            .with(
                11,
                dict(&[
                    ("S", Object::name("P")),
                    ("Pg", Object::Ref(page1)),
                    (
                        "K",
                        Object::Array(vec![
                            Object::Int(2),
                            dict(&[("Type", Object::name("MCR")), ("MCID", Object::Int(0))]),
                        ]),
                    ),
                ]),
            )
            .with(
                12,
                dict(&[
                    ("S", Object::name("P")),
                    ("Pg", Object::Ref(page2)),
                    ("K", Object::Int(9)),
                ]),
            );

        let mut catalog = Dict::new();
        catalog.set("StructTreeRoot", Object::Ref(Ref::new(10, 0)));

        assert_eq!(page_reading_mcids(&resolver, &catalog, page1), vec![2, 0]);
        assert_eq!(page_reading_mcids(&resolver, &catalog, page2), vec![9]);
    }

    #[test]
    fn cyclic_struct_tree_terminates() {
        let page1 = Ref::new(5, 0);
        let resolver = MapResolver::new()
            .with(
                10,
                dict(&[
                    ("Type", Object::name("StructTreeRoot")),
                    ("K", Object::Ref(Ref::new(11, 0))),
                ]),
            )
            .with(
                11,
                dict(&[
                    ("S", Object::name("Sect")),
                    ("Pg", Object::Ref(page1)),
                    (
                        "K",
                        Object::Array(vec![Object::Int(1), Object::Ref(Ref::new(11, 0))]),
                    ),
                ]),
            );
        let mut catalog = Dict::new();
        catalog.set("StructTreeRoot", Object::Ref(Ref::new(10, 0)));

        assert_eq!(page_reading_mcids(&resolver, &catalog, page1), vec![1]);
    }
}
