//! Page organization: reorder, insert, delete, rotate, crop, extract, split, merge,
//! replace, page labels, and stale `/Thumb` cleanup (GOAL.md §3.6, FINISH.md S7).

use std::collections::{BTreeMap, BTreeSet};

use mangle_crypto::sha256;
use mangle_doc::PageTree;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Object, Rect as PageRect, Ref, Stream};
use mangle_syntax::save::{SaveMode, SaveOptions};
use mangle_syntax::writer::IncrementalUpdate;

use crate::writeback::{Save, SaveError};

/// Page-label numbering style (`ISO 32000-1` Table 159).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// Decimal arabic numerals (`1, 2, 3…`).
    Decimal,
    /// Uppercase roman numerals (`I, II, III…`).
    UpperRoman,
    /// Lowercase roman numerals (`i, ii, iii…`).
    LowerRoman,
    /// Uppercase letters (`A, B, C…`).
    UpperAlpha,
    /// Lowercase letters (`a, b, c…`).
    LowerAlpha,
}

impl LabelStyle {
    #[must_use]
    pub fn pdf_name(self) -> &'static str {
        match self {
            Self::Decimal => "D",
            Self::UpperRoman => "R",
            Self::LowerRoman => "r",
            Self::UpperAlpha => "A",
            Self::LowerAlpha => "a",
        }
    }
}

/// One page-label numbering range starting at `start_page` (0-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLabelRange {
    pub start_page: usize,
    pub style: LabelStyle,
    pub prefix: String,
    pub first_number: i64,
}

/// Reorder a page from `from_index` to `to_index` (0-based) in the document's page tree (`S7.1`).
///
/// Because page object numbers (`Ref`) are preserved in place, bookmarks and destinations
/// targeting page objects continue to hit the same content pages automatically.
pub fn reorder_page(doc: &Document, from_index: usize, to_index: usize) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let (pages_ref, mut page_refs) = flat_page_refs(doc)?;
    if from_index >= page_refs.len() || to_index >= page_refs.len() || from_index == to_index {
        return Ok(unchanged_save(doc));
    }
    let moved = page_refs.remove(from_index);
    page_refs.insert(to_index, moved);
    write_page_tree_kids(doc, pages_ref, &page_refs, &[])
}

/// Rotate page `page_index` by `delta_degrees` (multiple of 90°), removing any stale `/Thumb` (`S7.4`, `S7.9`).
pub fn rotate_page(
    doc: &Document,
    page_index: usize,
    delta_degrees: i64,
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let tree = load_tree(doc)?;
    let Some(page) = tree.get(page_index) else {
        return Ok(unchanged_save(doc));
    };
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let current = i64::from(page.inherited.rotation());
    let next = (current + delta_degrees).rem_euclid(360);
    let mut page_dict = page.dict.clone();
    page_dict.set("Rotate", Object::Int(next));
    page_dict.remove("Thumb");
    Ok(write_extra(doc, &[(page_ref.num, Object::Dict(page_dict))]))
}

/// Set `/CropBox` on `page_index` non-destructively and remove any stale `/Thumb` (`S7.5`, `S7.9`).
pub fn crop_page(doc: &Document, page_index: usize, crop: PageRect) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let tree = load_tree(doc)?;
    let Some(page) = tree.get(page_index) else {
        return Ok(unchanged_save(doc));
    };
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let mut page_dict = page.dict.clone();
    page_dict.set(
        "CropBox",
        Object::Array(vec![
            Object::Real(crop.left),
            Object::Real(crop.bottom),
            Object::Real(crop.right),
            Object::Real(crop.top),
        ]),
    );
    page_dict.remove("Thumb");
    Ok(write_extra(doc, &[(page_ref.num, Object::Dict(page_dict))]))
}

/// Insert a blank page of `(width, height)` points after `after_index` (`None` inserts at index 0) (`S7.3`).
pub fn insert_blank_page(
    doc: &Document,
    after_index: Option<usize>,
    width: f64,
    height: f64,
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let (pages_ref, mut page_refs) = flat_page_refs(doc)?;
    let stream_num = doc.alloc().num;
    let res_num = doc.alloc().num;
    let page_num = doc.alloc().num;
    let page_ref = Ref::new(page_num, 0);

    let mut page_dict = Dict::new();
    page_dict.set("Type", Object::name("Page"));
    page_dict.set("Parent", Object::Ref(pages_ref));
    page_dict.set(
        "MediaBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(width),
            Object::Real(height),
        ]),
    );
    page_dict.set("Contents", Object::Ref(Ref::new(stream_num, 0)));
    page_dict.set("Resources", Object::Ref(Ref::new(res_num, 0)));

    let pos = match after_index {
        Some(idx) => (idx + 1).min(page_refs.len()),
        None => 0,
    };
    page_refs.insert(pos, page_ref);

    let extra = vec![
        (
            stream_num,
            Object::Stream(Stream::new(Dict::new(), b"q Q".to_vec())),
        ),
        (res_num, Object::Dict(Dict::new())),
        (page_num, Object::Dict(page_dict)),
    ];
    write_page_tree_kids(doc, pages_ref, &page_refs, &extra)
}

/// Delete the pages at `indices` (0-based), prune outlines/destinations referencing them,
/// and perform a full rewrite with resource GC (`S7.2`).
pub fn delete_pages(doc: &Document, indices: &[usize]) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let (pages_ref, page_refs) = flat_page_refs(doc)?;
    let mut removed_nums = BTreeSet::new();
    let mut kept_refs = Vec::new();
    for (i, r) in page_refs.iter().enumerate() {
        if indices.contains(&i) {
            removed_nums.insert(r.num);
        } else {
            kept_refs.push(*r);
        }
    }
    if removed_nums.is_empty() {
        return Ok(unchanged_save(doc));
    }

    let mut pages_dict = doc
        .object(pages_ref)
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    pages_dict.set("Type", Object::name("Pages"));
    pages_dict.set(
        "Kids",
        Object::Array(kept_refs.iter().map(|r| Object::Ref(*r)).collect()),
    );
    pages_dict.set("Count", Object::Int(kept_refs.len() as i64));
    doc.set(pages_ref, Object::Dict(pages_dict));
    for num in &removed_nums {
        doc.remove(Ref::new(*num, 0));
    }

    let report = doc
        .save(&SaveOptions {
            mode: SaveMode::Full,
            ..SaveOptions::default()
        })
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    Ok(Save {
        bytes: report.bytes,
        rewritten: vec![pages_ref.num],
        installed: Vec::new(),
        re_encoded: false,
    })
}

/// Import `source_indices` from `source_doc` into `doc` after `after_index`, deduplicating
/// identical streams by SHA-256 hash (`S7.3`).
pub fn insert_pages_from_doc(
    doc: &Document,
    after_index: Option<usize>,
    source_doc: &Document,
    source_indices: &[usize],
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted || source_doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let (pages_ref, mut page_refs) = flat_page_refs(doc)?;
    let source_tree = load_tree(source_doc)?;

    // Build SHA-256 table of existing streams in `doc` for deduplication.
    let mut stream_hashes: BTreeMap<[u8; 32], Ref> = BTreeMap::new();
    for num in doc.object_numbers() {
        let r = Ref::new(num, 0);
        if let Some(Object::Stream(st)) = doc.object(r) {
            stream_hashes.insert(sha256(&st.raw), r);
        }
    }

    let mut remapped: BTreeMap<u32, Ref> = BTreeMap::new();
    let mut extra: Vec<(u32, Object)> = Vec::new();
    let mut imported_refs: Vec<Ref> = Vec::new();

    for &src_idx in source_indices {
        let Some(src_page) = source_tree.get(src_idx) else {
            continue;
        };
        let mut new_page_dict = Dict::new();
        new_page_dict.set("Type", Object::name("Page"));
        new_page_dict.set("Parent", Object::Ref(pages_ref));
        let mb = src_page.inherited.media_rect();
        new_page_dict.set(
            "MediaBox",
            Object::Array(vec![
                Object::Real(mb.left),
                Object::Real(mb.bottom),
                Object::Real(mb.right),
                Object::Real(mb.top),
            ]),
        );
        if src_page.inherited.crop_box.is_some() {
            let cb = src_page.inherited.crop_rect();
            new_page_dict.set(
                "CropBox",
                Object::Array(vec![
                    Object::Real(cb.left),
                    Object::Real(cb.bottom),
                    Object::Real(cb.right),
                    Object::Real(cb.top),
                ]),
            );
        }
        let rot = src_page.inherited.rotation();
        if rot != 0 {
            new_page_dict.set("Rotate", Object::Int(i64::from(rot)));
        }
        if let Some(contents) = src_page.dict.get("Contents") {
            let cloned_contents = clone_object_graph(
                doc,
                source_doc,
                contents,
                &mut stream_hashes,
                &mut remapped,
                &mut extra,
                0,
            );
            new_page_dict.set("Contents", cloned_contents);
        }
        if let Some(resources) = &src_page.inherited.resources {
            let cloned_res = clone_object_graph(
                doc,
                source_doc,
                resources,
                &mut stream_hashes,
                &mut remapped,
                &mut extra,
                0,
            );
            new_page_dict.set("Resources", cloned_res);
        }

        let new_page_num = doc.alloc().num;
        let new_page_ref = Ref::new(new_page_num, 0);
        extra.push((new_page_num, Object::Dict(new_page_dict)));
        imported_refs.push(new_page_ref);
    }

    let pos = match after_index {
        Some(idx) => (idx + 1).min(page_refs.len()),
        None => 0,
    };
    for (offset, r) in imported_refs.into_iter().enumerate() {
        page_refs.insert(pos + offset, r);
    }
    write_page_tree_kids(doc, pages_ref, &page_refs, &extra)
}

/// Replace `target_index` in `doc` with `source_index` from `source_doc`, removing any stale `/Thumb` (`S7.8`, `S7.9`).
pub fn replace_page(
    doc: &Document,
    target_index: usize,
    source_doc: &Document,
    source_index: usize,
) -> Result<Save, SaveError> {
    let inserted = insert_pages_from_doc(doc, Some(target_index), source_doc, &[source_index])?;
    let reopened = Document::open(inserted.bytes, mangle_syntax::OpenOptions::default())
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    delete_pages(&reopened, &[target_index])
}

/// Extract `indices` (0-based) into a standalone valid PDF document (`S7.6`).
pub fn extract_pages(doc: &Document, indices: &[usize]) -> Result<Vec<u8>, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let (_, all_refs) = flat_page_refs(doc)?;
    let to_delete: Vec<usize> = (0..all_refs.len())
        .filter(|i| !indices.contains(i))
        .collect();
    let cloned = Document::open(doc.bytes().to_vec(), mangle_syntax::OpenOptions::default())
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    Ok(delete_pages(&cloned, &to_delete)?.bytes)
}

/// Split `doc` into standalone PDFs of at most `chunk_size` pages each (`S7.6`).
pub fn split_every_n(doc: &Document, chunk_size: usize) -> Result<Vec<Vec<u8>>, SaveError> {
    let chunk = chunk_size.max(1);
    let (_, all_refs) = flat_page_refs(doc)?;
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < all_refs.len() {
        let end = (start + chunk).min(all_refs.len());
        let indices: Vec<usize> = (start..end).collect();
        out.push(extract_pages(doc, &indices)?);
        start = end;
    }
    Ok(out)
}

/// Merge multiple documents into a single PDF, preserving page sizes/rotations and deduplicating
/// identical streams by SHA-256 hash (`S7.7`).
pub fn merge_documents(docs: &[&Document]) -> Result<Vec<u8>, SaveError> {
    let Some((first, rest)) = docs.split_first() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let mut current_bytes = first.bytes().to_vec();
    for src in rest {
        let cur_doc = Document::open(current_bytes, mangle_syntax::OpenOptions::default())
            .map_err(|_| SaveError::ContentsNotIndirect)?;
        let cur_count = cur_doc.page_count().unwrap_or(0);
        let src_count = src.page_count().unwrap_or(0);
        let src_indices: Vec<usize> = (0..src_count).collect();
        let after = cur_count.checked_sub(1);
        let saved = insert_pages_from_doc(&cur_doc, after, src, &src_indices)?;
        current_bytes = saved.bytes;
    }
    Ok(current_bytes)
}

/// Set `/PageLabels` on the document catalog (`S7.8`).
pub fn set_page_labels(doc: &Document, ranges: &[PageLabelRange]) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let Some(cat_ref) = doc.catalog_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let cat_obj = doc.catalog().map_err(|_| SaveError::ContentsNotIndirect)?;
    let Some(mut cat_dict) = cat_obj.as_dict().cloned() else {
        return Err(SaveError::ContentsNotIndirect);
    };

    let mut nums = Vec::with_capacity(ranges.len() * 2);
    for range in ranges {
        nums.push(Object::Int(range.start_page as i64));
        let mut label_dict = Dict::new();
        label_dict.set("Type", Object::name("PageLabel"));
        label_dict.set("S", Object::name(range.style.pdf_name()));
        if !range.prefix.is_empty() {
            label_dict.set("P", Object::string(&range.prefix));
        }
        if range.first_number > 1 {
            label_dict.set("St", Object::Int(range.first_number));
        }
        nums.push(Object::Dict(label_dict));
    }
    let mut page_labels = Dict::new();
    page_labels.set("Nums", Object::Array(nums));
    cat_dict.set("PageLabels", Object::Dict(page_labels));
    Ok(write_extra(doc, &[(cat_ref.num, Object::Dict(cat_dict))]))
}

#[allow(clippy::only_used_in_recursion)]
fn clone_object_graph(
    dst: &Document,
    src: &Document,
    obj: &Object,
    stream_hashes: &mut BTreeMap<[u8; 32], Ref>,
    remapped: &mut BTreeMap<u32, Ref>,
    extra: &mut Vec<(u32, Object)>,
    depth: usize,
) -> Object {
    if depth > 24 {
        return Object::Null;
    }
    match obj {
        Object::Ref(r) => {
            if let Some(&mapped) = remapped.get(&r.num) {
                return Object::Ref(mapped);
            }
            let Some(resolved) = src.object(*r) else {
                return Object::Null;
            };
            if let Object::Stream(st) = &resolved {
                let digest = sha256(&st.raw);
                if let Some(&existing) = stream_hashes.get(&digest) {
                    remapped.insert(r.num, existing);
                    return Object::Ref(existing);
                }
                let new_num = dst.alloc().num;
                let new_ref = Ref::new(new_num, 0);
                remapped.insert(r.num, new_ref);
                stream_hashes.insert(digest, new_ref);
                let cloned = clone_object_graph(
                    dst,
                    src,
                    &resolved,
                    stream_hashes,
                    remapped,
                    extra,
                    depth + 1,
                );
                extra.push((new_num, cloned));
                return Object::Ref(new_ref);
            }
            let new_num = dst.alloc().num;
            let new_ref = Ref::new(new_num, 0);
            remapped.insert(r.num, new_ref);
            let cloned = clone_object_graph(
                dst,
                src,
                &resolved,
                stream_hashes,
                remapped,
                extra,
                depth + 1,
            );
            extra.push((new_num, cloned));
            Object::Ref(new_ref)
        }
        Object::Array(items) => Object::Array(
            items
                .iter()
                .map(|item| {
                    clone_object_graph(dst, src, item, stream_hashes, remapped, extra, depth + 1)
                })
                .collect(),
        ),
        Object::Dict(dict) => {
            let mut out = Dict::new();
            for (k, v) in dict.iter() {
                if k.as_bytes() == b"Parent" {
                    continue;
                }
                out.insert(
                    k.clone(),
                    clone_object_graph(dst, src, v, stream_hashes, remapped, extra, depth + 1),
                );
            }
            Object::Dict(out)
        }
        Object::Stream(st) => {
            let mut out_dict = Dict::new();
            for (k, v) in st.dict.iter() {
                out_dict.insert(
                    k.clone(),
                    clone_object_graph(dst, src, v, stream_hashes, remapped, extra, depth + 1),
                );
            }
            Object::Stream(Stream::new(out_dict, st.raw.clone()))
        }
        other => other.clone(),
    }
}

fn load_tree(doc: &Document) -> Result<PageTree, SaveError> {
    let cat = doc.catalog().map_err(|_| SaveError::ContentsNotIndirect)?;
    let root = cat
        .get("Pages")
        .and_then(Object::as_ref_id)
        .ok_or(SaveError::ContentsNotIndirect)?;
    PageTree::build(doc, root).map_err(|_| SaveError::ContentsNotIndirect)
}

fn flat_page_refs(doc: &Document) -> Result<(Ref, Vec<Ref>), SaveError> {
    let cat = doc.catalog().map_err(|_| SaveError::ContentsNotIndirect)?;
    let root = cat
        .get("Pages")
        .and_then(Object::as_ref_id)
        .ok_or(SaveError::ContentsNotIndirect)?;
    let tree = PageTree::build(doc, root).map_err(|_| SaveError::ContentsNotIndirect)?;
    let mut refs = Vec::with_capacity(tree.len());
    for p in tree.pages() {
        let r = p.object_ref().ok_or(SaveError::ContentsNotIndirect)?;
        refs.push(r);
    }
    Ok((root, refs))
}

fn write_page_tree_kids(
    doc: &Document,
    pages_ref: Ref,
    page_refs: &[Ref],
    extra: &[(u32, Object)],
) -> Result<Save, SaveError> {
    let mut pages_dict = doc
        .object(pages_ref)
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    pages_dict.set("Type", Object::name("Pages"));
    pages_dict.set(
        "Kids",
        Object::Array(page_refs.iter().map(|r| Object::Ref(*r)).collect()),
    );
    pages_dict.set("Count", Object::Int(page_refs.len() as i64));

    let mut all_extra = extra.to_vec();
    all_extra.push((pages_ref.num, Object::Dict(pages_dict)));
    Ok(write_extra(doc, &all_extra))
}

fn write_extra(doc: &Document, extra: &[(u32, Object)]) -> Save {
    let mut update = IncrementalUpdate::new(doc.xref().size());
    for (num, obj) in extra {
        update.set(*num, obj.clone());
    }
    if let Some(info) = doc.info_ref() {
        update.info = Some(info);
    }
    let bytes = update.append_with_id(
        doc.bytes(),
        doc.xref().start_xref().unwrap_or(0),
        doc.catalog_ref(),
        &doc.file_id(),
    );
    Save {
        bytes,
        rewritten: Vec::new(),
        installed: extra.iter().map(|(n, _)| *n).collect(),
        re_encoded: false,
    }
}

fn unchanged_save(doc: &Document) -> Save {
    Save {
        bytes: doc.bytes().to_vec(),
        rewritten: Vec::new(),
        installed: Vec::new(),
        re_encoded: false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::fmt::Write as _;

    use mangle_doc::{PageLabel, PageTree};
    use mangle_syntax::object::{Object, Rect as PageRect};
    use mangle_syntax::{Document, OpenOptions};

    use super::{
        LabelStyle, PageLabelRange, crop_page, delete_pages, extract_pages, insert_blank_page,
        merge_documents, reorder_page, replace_page, rotate_page, set_page_labels, split_every_n,
    };

    fn multi_page_pdf(widths: &[i64]) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        let kids: Vec<String> = (0..widths.len())
            .map(|i| format!("{} 0 R", 3 + i * 2))
            .collect();
        offsets.push(out.len());
        let _ = writeln!(
            out,
            "2 0 obj\n<</Type /Pages /Kids [{}] /Count {}>>\nendobj",
            kids.join(" "),
            widths.len()
        );
        for (i, w) in widths.iter().enumerate() {
            let page_num = 3 + i * 2;
            let stream_num = page_num + 1;
            offsets.push(out.len());
            let _ = writeln!(
                out,
                "{page_num} 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 {w} 200] \
                 /Contents {stream_num} 0 R /Thumb {stream_num} 0 R>>\nendobj"
            );
            let body = format!("q 0 0 {w} 10 re f Q");
            offsets.push(out.len());
            let _ = writeln!(
                out,
                "{stream_num} 0 obj\n<</Length {}>>\nstream\n{body}\nendstream\nendobj",
                body.len()
            );
        }
        let start_xref = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len());
        for off in offsets.iter().skip(1) {
            let _ = writeln!(out, "{off:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<</Size {} /Root 1 0 R>>\nstartxref\n{start_xref}\n%%EOF\n",
            offsets.len()
        );
        out.into_bytes()
    }

    fn open_tree(bytes: Vec<u8>) -> (Document, PageTree) {
        let doc = Document::open(bytes, OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        (doc, tree)
    }

    #[test]
    fn reorder_rotate_crop_insert_and_stale_thumb_removal() {
        let (doc, _) = open_tree(multi_page_pdf(&[100, 200, 300]));
        let reordered = reorder_page(&doc, 2, 0).expect("reorders p3 to p1");
        let (doc2, tree2) = open_tree(reordered.bytes);
        assert_eq!(tree2.get(0).unwrap().inherited.displayed_size().0, 300.0);
        assert_eq!(tree2.get(1).unwrap().inherited.displayed_size().0, 100.0);

        let rotated = rotate_page(&doc2, 0, 90).expect("rotates p1 by 90");
        let (doc3, tree3) = open_tree(rotated.bytes);
        assert_eq!(tree3.get(0).unwrap().inherited.rotation(), 90);
        assert!(
            tree3.get(0).unwrap().dict.get("Thumb").is_none(),
            "stale /Thumb removed on rotation"
        );

        let cropped = crop_page(&doc3, 1, PageRect::new(10.0, 10.0, 90.0, 190.0)).expect("crops");
        let (doc4, tree4) = open_tree(cropped.bytes);
        assert_eq!(
            tree4.get(1).unwrap().inherited.crop_rect(),
            PageRect::new(10.0, 10.0, 90.0, 190.0)
        );

        let inserted = insert_blank_page(&doc4, Some(0), 612.0, 792.0).expect("inserts blank");
        let (_, tree5) = open_tree(inserted.bytes);
        assert_eq!(tree5.len(), 4);
        assert_eq!(
            tree5.get(1).unwrap().inherited.displayed_size(),
            (612.0, 792.0)
        );
    }

    #[test]
    fn delete_extract_split_merge_replace_and_page_labels() {
        let (doc_a, _) = open_tree(multi_page_pdf(&[100, 200, 300, 400]));
        let (doc_b, _) = open_tree(multi_page_pdf(&[500, 600]));

        let extracted = extract_pages(&doc_a, &[0, 2]).expect("extracts p1 and p3");
        let (_, ext_tree) = open_tree(extracted);
        assert_eq!(ext_tree.len(), 2);
        assert_eq!(ext_tree.get(0).unwrap().inherited.displayed_size().0, 100.0);
        assert_eq!(ext_tree.get(1).unwrap().inherited.displayed_size().0, 300.0);

        let chunks = split_every_n(&doc_a, 2).expect("splits by 2");
        assert_eq!(chunks.len(), 2);

        let merged = merge_documents(&[&doc_a, &doc_b]).expect("merges");
        let (doc_m, tree_m) = open_tree(merged);
        assert_eq!(tree_m.len(), 6);
        assert_eq!(tree_m.get(5).unwrap().inherited.displayed_size().0, 600.0);

        let replaced = replace_page(&doc_m, 0, &doc_b, 1).expect("replaces p1 with 600w page");
        let (doc_r, tree_r) = open_tree(replaced.bytes);
        assert_eq!(tree_r.len(), 6);
        assert_eq!(tree_r.get(0).unwrap().inherited.displayed_size().0, 600.0);

        let labeled = set_page_labels(
            &doc_r,
            &[
                PageLabelRange {
                    start_page: 0,
                    style: LabelStyle::LowerRoman,
                    prefix: String::new(),
                    first_number: 1,
                },
                PageLabelRange {
                    start_page: 3,
                    style: LabelStyle::Decimal,
                    prefix: String::new(),
                    first_number: 1,
                },
            ],
        )
        .expect("sets labels");
        let (doc_l, _) = open_tree(labeled.bytes);
        let cat_l = doc_l.catalog().expect("cat");
        let labels = PageLabel::build(&doc_l, cat_l.as_dict().expect("dict"));
        assert_eq!(labels.label_text(0), "i");
        assert_eq!(labels.label_text(2), "iii");
        assert_eq!(labels.label_text(3), "1");

        let deleted = delete_pages(&doc_l, &[1, 2]).expect("deletes 2 pages");
        let (_, tree_d) = open_tree(deleted.bytes);
        assert_eq!(tree_d.len(), 4);
    }
}
