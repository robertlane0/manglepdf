//! Annotation and form flattening (ISO 32000-1 §12.5.5, GOAL.md §5.5).

use std::collections::BTreeSet;
use std::fmt;

use mangle_content::matrix::Matrix;
use mangle_doc::Page;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Name, Object, Rect as PageRect, Ref, Stream};
use mangle_syntax::writer::IncrementalUpdate;

use crate::writeback::{Save, SaveError, save_decoded_with};

/// `/F` bit 2: Hidden.
const ANNOT_HIDDEN: i64 = 1 << 1;
/// `/F` bit 3: Print.
const ANNOT_PRINT: i64 = 1 << 2;
/// `/F` bit 6: NoView.
const ANNOT_NO_VIEW: i64 = 1 << 5;

/// Which annotations to flatten on a page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FlattenFilter {
    /// Flatten all eligible annotations.
    #[default]
    All,
    /// Flatten markup annotations while keeping `/Link` and `/Widget` annotations.
    MarkupOnly,
    /// Flatten only `/Widget` form fields.
    FormsOnly,
    /// Flatten only the annotations at these 0-based indices in `/Annots`.
    Indices(Vec<usize>),
    /// Flatten only annotations whose `/Subtype` is in this list.
    Subtypes(Vec<String>),
}

/// Options controlling annotation flattening.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlattenOptions {
    /// Which subset of annotations to flatten.
    pub filter: FlattenFilter,
    /// Keep `/Link` annotations interactive even under [`FlattenFilter::All`].
    pub keep_links: bool,
    /// Keep `/Widget` form annotations interactive even under [`FlattenFilter::All`].
    pub keep_forms: bool,
    /// Flatten annotations marked `Hidden` or `NoView` instead of skipping them.
    pub include_hidden: bool,
    /// Flatten print-only annotations (`Print` + `NoView`) during screen flatten.
    pub include_print_only: bool,
    /// Confirm flattening on a digitally signed document (invalidates signatures).
    pub allow_signed: bool,
}

/// Outcome of flattening a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenReport {
    /// Incremental save containing the flattened page content and updated `/Annots`.
    pub save: Save,
    /// Number of annotations flattened into page content.
    pub flattened: usize,
    /// Number of `Hidden` or `NoView` annotations skipped.
    pub skipped_hidden: usize,
    /// Number of annotations kept in `/Annots` by filter options.
    pub kept: usize,
    /// Whether the document carried a digital signature that required confirmation.
    pub signed_warning: bool,
}

/// Why flattening was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlattenError {
    /// Document is digitally signed and `allow_signed` was `false`.
    SignedDocumentNeedsConfirmation,
    /// Page dictionary is not an indirect object.
    DirectPage,
    /// Underlying save failed.
    Save(SaveError),
}

impl fmt::Display for FlattenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignedDocumentNeedsConfirmation => write!(
                f,
                "this document is digitally signed; flattening modifies page content and \
                 invalidates existing signatures unless confirmed"
            ),
            Self::DirectPage => write!(
                f,
                "the page has no indirect object number, so its `/Annots` cannot be updated"
            ),
            Self::Save(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FlattenError {}

impl From<SaveError> for FlattenError {
    fn from(e: SaveError) -> Self {
        Self::Save(e)
    }
}

/// Check whether `doc` contains a signed `/Sig` field or signature dictionary.
#[must_use]
pub fn has_digital_signature(doc: &Document) -> bool {
    let Ok(catalog) = doc.catalog() else {
        return false;
    };
    let Some(cat_dict) = catalog.as_dict() else {
        return false;
    };
    if let Some(acro) = cat_dict
        .get("AcroForm")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
        && let Some(fields) = acro
            .get("Fields")
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_array().map(<[Object]>::to_vec))
    {
        for field in &fields {
            if is_signed_field(doc, field) {
                return true;
            }
        }
    }
    for num in doc.object_numbers() {
        if let Some(obj) = doc.object(Ref::new(num, 0))
            && let Some(d) = obj.as_dict()
            && d.get("ByteRange").is_some()
            && d.get("Contents").is_some()
        {
            return true;
        }
    }
    false
}

fn is_signed_field(doc: &Document, field: &Object) -> bool {
    let Some(resolved) = doc.resolve_object(field).or_else(|| Some(field.clone())) else {
        return false;
    };
    let Some(dict) = resolved.as_dict() else {
        return false;
    };
    if dict.get("FT").and_then(Object::as_name) == Some(b"Sig") && dict.get("V").is_some() {
        return true;
    }
    if dict.get("ByteRange").is_some() && dict.get("Contents").is_some() {
        return true;
    }
    if let Some(kids) = dict
        .get("Kids")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
    {
        for kid in &kids {
            if is_signed_field(doc, kid) {
                return true;
            }
        }
    }
    false
}

/// Flatten eligible annotations on `page` into the page's content stream.
pub fn flatten_page(
    doc: &Document,
    page: &Page,
    options: &FlattenOptions,
) -> Result<FlattenReport, FlattenError> {
    if doc.info().encryption.encrypted {
        return Err(FlattenError::Save(SaveError::Encrypted));
    }
    let signed_warning = has_digital_signature(doc);
    if signed_warning && !options.allow_signed {
        return Err(FlattenError::SignedDocumentNeedsConfirmation);
    }
    let page_num = page.object_ref().ok_or(FlattenError::DirectPage)?.num;

    let raw_annots = page
        .dict
        .get("Annots")
        .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();

    if raw_annots.is_empty() {
        return Ok(FlattenReport {
            save: Save {
                bytes: doc.bytes().to_vec(),
                rewritten: Vec::new(),
                installed: Vec::new(),
                re_encoded: false,
            },
            flattened: 0,
            skipped_hidden: 0,
            kept: 0,
            signed_warning,
        });
    }

    let mut skipped_hidden = 0usize;
    let mut kept = 0usize;
    let mut flattened = 0usize;
    let mut flattened_refs: BTreeSet<u32> = BTreeSet::new();
    let mut flattened_widgets = false;
    let mut to_bake: Vec<(Stream, Matrix)> = Vec::new();
    let mut first_pass_kept: Vec<Object> = Vec::new();

    for (index, entry) in raw_annots.iter().enumerate() {
        let ref_num = entry.as_ref_id().map(|r| r.num);
        let Some(resolved) = doc.resolve_object(entry).or_else(|| Some(entry.clone())) else {
            continue;
        };
        let Some(dict) = resolved.as_dict() else {
            continue;
        };
        let subtype = dict
            .get("Subtype")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();

        // Popup annotations are pruned in the second pass if their parent was flattened.
        if subtype == "Popup" {
            first_pass_kept.push(entry.clone());
            continue;
        }

        let flags = dict.get("F").and_then(Object::as_i64).unwrap_or(0);
        let is_hidden = (flags & ANNOT_HIDDEN) != 0;
        let is_no_view = (flags & ANNOT_NO_VIEW) != 0;
        let is_print = (flags & ANNOT_PRINT) != 0;

        if is_hidden && !options.include_hidden {
            skipped_hidden += 1;
            first_pass_kept.push(entry.clone());
            continue;
        }
        if is_no_view {
            let allow = if is_print {
                options.include_print_only || options.include_hidden
            } else {
                options.include_hidden
            };
            if !allow {
                skipped_hidden += 1;
                first_pass_kept.push(entry.clone());
                continue;
            }
        }

        if !matches_filter(&subtype, index, options) {
            kept += 1;
            first_pass_kept.push(entry.clone());
            continue;
        }

        if let Some((form, matrix)) = extract_appearance(doc, dict) {
            to_bake.push((form, matrix));
        }
        flattened += 1;
        if subtype == "Widget" {
            flattened_widgets = true;
        }
        if let Some(num) = ref_num {
            flattened_refs.insert(num);
        }
    }

    if flattened == 0 {
        return Ok(FlattenReport {
            save: Save {
                bytes: doc.bytes().to_vec(),
                rewritten: Vec::new(),
                installed: Vec::new(),
                re_encoded: false,
            },
            flattened: 0,
            skipped_hidden,
            kept,
            signed_warning,
        });
    }

    // Second pass: remove popups and `/IRT` replies whose parent annotation was flattened.
    let mut remaining_annots: Vec<Object> = Vec::new();
    for entry in first_pass_kept {
        let Some(resolved) = doc.resolve_object(&entry).or_else(|| Some(entry.clone())) else {
            continue;
        };
        let Some(dict) = resolved.as_dict() else {
            continue;
        };
        let subtype = dict
            .get("Subtype")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        let parent_flattened = dict
            .get("Parent")
            .and_then(Object::as_ref_id)
            .is_some_and(|r| flattened_refs.contains(&r.num));
        let irt_flattened = dict
            .get("IRT")
            .and_then(Object::as_ref_id)
            .is_some_and(|r| flattened_refs.contains(&r.num));
        if (subtype == "Popup" && parent_flattened) || irt_flattened {
            kept = kept.saturating_sub(1);
            continue;
        }
        remaining_annots.push(entry);
    }

    // Build Form XObjects and append `q <matrix> cm /FmN Do Q` operators.
    let mut res_dict = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    let mut xobject_table = res_dict
        .get("XObject")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();

    let mut extra: Vec<(u32, Object)> = Vec::new();
    let mut snippet = Vec::new();
    for (ap_stream, matrix) in to_bake {
        let fm_name = next_xobject_name(&xobject_table);
        let fm_num = doc.alloc().num;
        let mut fm_dict = Dict::new();
        for (k, v) in ap_stream.dict.iter() {
            if matches!(k.as_bytes(), b"Matrix" | b"Transform" | b"BBox") {
                continue;
            }
            fm_dict.insert(k.clone(), v.clone());
        }
        fm_dict.set("Type", Object::name("XObject"));
        fm_dict.set("Subtype", Object::name("Form"));
        fm_dict.set(
            "BBox",
            Object::Array(vec![
                Object::Int(-100_000),
                Object::Int(-100_000),
                Object::Int(100_000),
                Object::Int(100_000),
            ]),
        );
        let fm_stream = Stream::new(fm_dict, ap_stream.raw);
        extra.push((fm_num, Object::Stream(fm_stream)));
        xobject_table.insert(Name::new(&fm_name), Object::Ref(Ref::new(fm_num, 0)));

        let cmd = format!(
            "\nq {} {} {} {} {} {} cm /{fm_name} Do Q",
            fmt_num(matrix.a),
            fmt_num(matrix.b),
            fmt_num(matrix.c),
            fmt_num(matrix.d),
            fmt_num(matrix.e),
            fmt_num(matrix.f),
        );
        snippet.extend_from_slice(cmd.as_bytes());
    }

    res_dict.set("XObject", Object::Dict(xobject_table));

    let mut page_dict = page.dict.clone();
    if remaining_annots.is_empty() {
        page_dict.remove("Annots");
    } else {
        page_dict.set("Annots", Object::Array(remaining_annots));
    }

    match &page.inherited.resources {
        Some(Object::Ref(r)) => {
            extra.push((r.num, Object::Dict(res_dict)));
        }
        _ => {
            let res_num = doc.alloc().num;
            extra.push((res_num, Object::Dict(res_dict)));
            page_dict.set("Resources", Object::Ref(Ref::new(res_num, 0)));
        }
    }

    // If Widget annotations were flattened, prune them from `/AcroForm` on the catalog.
    if flattened_widgets
        && let Some(cat_ref) = doc.catalog_ref()
        && let Ok(cat_obj) = doc.catalog()
        && let Some(mut cat_dict) = cat_obj.as_dict().cloned()
    {
        if let Some(acro_obj) = cat_dict.get("AcroForm").cloned() {
            let acro_ref = acro_obj.as_ref_id();
            if let Some(mut acro_dict) = doc
                .resolve_object(&acro_obj)
                .or(Some(acro_obj))
                .and_then(|o| o.as_dict().cloned())
            {
                let remaining_fields: Vec<Object> = acro_dict
                    .get("Fields")
                    .and_then(|f| doc.resolve_object(f).or_else(|| Some(f.clone())))
                    .and_then(|f| f.as_array().map(<[Object]>::to_vec))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| {
                        !f.as_ref_id()
                            .is_some_and(|r| flattened_refs.contains(&r.num))
                    })
                    .collect();
                if remaining_fields.is_empty() {
                    cat_dict.remove("AcroForm");
                    extra.push((cat_ref.num, Object::Dict(cat_dict)));
                } else {
                    acro_dict.set("Fields", Object::Array(remaining_fields));
                    if let Some(r) = acro_ref {
                        extra.push((r.num, Object::Dict(acro_dict)));
                    } else {
                        cat_dict.set("AcroForm", Object::Dict(acro_dict));
                        extra.push((cat_ref.num, Object::Dict(cat_dict)));
                    }
                }
            }
        }
    }

    // Append `snippet` to the page's content stream (or create one if the page had none).
    let parts = crate::writeback::parts_of(doc, page)?;
    if parts.is_empty() {
        if !snippet.is_empty() {
            let stream_num = doc.alloc().num;
            let trimmed = snippet.strip_prefix(b"\n").unwrap_or(&snippet).to_vec();
            extra.push((
                stream_num,
                Object::Stream(Stream::new(Dict::new(), trimmed)),
            ));
            page_dict.set("Contents", Object::Ref(Ref::new(stream_num, 0)));
        }
        extra.push((page_num, Object::Dict(page_dict)));
        let mut update = IncrementalUpdate::new(doc.xref().size());
        for (num, obj) in &extra {
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
        return Ok(FlattenReport {
            save: Save {
                bytes,
                rewritten: Vec::new(),
                installed: extra.iter().map(|(n, _)| *n).collect(),
                re_encoded: false,
            },
            flattened,
            skipped_hidden,
            kept,
            signed_warning,
        });
    }

    extra.push((page_num, Object::Dict(page_dict)));
    let mut wanted: Vec<Option<Vec<u8>>> = vec![None; parts.len()];
    if !snippet.is_empty()
        && let Some((last_idx, last_part)) = parts.iter().enumerate().next_back()
    {
        let mut updated = last_part.decoded.clone();
        updated.extend_from_slice(&snippet);
        if let Some(slot) = wanted.get_mut(last_idx) {
            *slot = Some(updated);
        }
    }
    let save = save_decoded_with(doc, page, &wanted, &extra)?;
    Ok(FlattenReport {
        save,
        flattened,
        skipped_hidden,
        kept,
        signed_warning,
    })
}

fn matches_filter(subtype: &str, index: usize, options: &FlattenOptions) -> bool {
    if subtype == "Link" && options.keep_links {
        return false;
    }
    if subtype == "Widget" && options.keep_forms {
        return false;
    }
    match &options.filter {
        FlattenFilter::All => true,
        FlattenFilter::MarkupOnly => subtype != "Link" && subtype != "Widget",
        FlattenFilter::FormsOnly => subtype == "Widget",
        FlattenFilter::Indices(indices) => indices.contains(&index),
        FlattenFilter::Subtypes(types) => types.iter().any(|t| t.eq_ignore_ascii_case(subtype)),
    }
}

fn extract_appearance(doc: &Document, dict: &Dict) -> Option<(Stream, Matrix)> {
    let ap = dict
        .get("AP")
        .and_then(|ap| doc.resolve_object(ap).or_else(|| Some(ap.clone())))?;
    let normal = ap
        .as_dict()
        .and_then(|d| d.get("N"))
        .and_then(|n| doc.resolve_object(n).or_else(|| Some(n.clone())))?;
    let chosen = match normal {
        Object::Dict(states) => {
            let wanted = dict
                .get("AS")
                .and_then(Object::as_name)
                .and_then(|n| std::str::from_utf8(n).ok());
            let found = wanted.and_then(|w| states.get(w))?;
            doc.resolve_object(found).or_else(|| Some(found.clone()))?
        }
        other => other,
    };
    let Object::Stream(form) = chosen else {
        return None;
    };
    let rect_obj = dict.get("Rect")?;
    let rect = PageRect::from_object(
        &doc.resolve_object(rect_obj)
            .unwrap_or_else(|| rect_obj.clone()),
    )
    .ok()?;
    if rect.right <= rect.left || rect.top <= rect.bottom {
        return None;
    }
    let bbox = match form.dict.get("BBox") {
        Some(b) => {
            PageRect::from_object(&doc.resolve_object(b).unwrap_or_else(|| b.clone())).ok()?
        }
        None => rect,
    };
    if bbox.right <= bbox.left || bbox.top <= bbox.bottom {
        return None;
    }
    let sx = (rect.right - rect.left) / (bbox.right - bbox.left);
    let sy = (rect.top - rect.bottom) / (bbox.top - bbox.bottom);
    let mut to_rect = Matrix::new(
        sx,
        0.0,
        0.0,
        sy,
        rect.left - bbox.left * sx,
        rect.bottom - bbox.bottom * sy,
    );
    if let Some(m) = form.dict.get("Matrix") {
        to_rect = to_rect.concat(read_matrix(Some(
            &doc.resolve_object(m).unwrap_or_else(|| m.clone()),
        )));
    }
    if let Some(t) = form.dict.get("Transform") {
        to_rect = to_rect.concat(read_matrix(Some(
            &doc.resolve_object(t).unwrap_or_else(|| t.clone()),
        )));
    }
    Some((form, to_rect))
}

fn read_matrix(object: Option<&Object>) -> Matrix {
    let Some(values) = object.and_then(Object::as_array) else {
        return Matrix::IDENTITY;
    };
    if values.len() != 6 {
        return Matrix::IDENTITY;
    }
    let at = |i: usize| values.get(i).and_then(Object::as_f64).unwrap_or(0.0);
    Matrix::new(at(0), at(1), at(2), at(3), at(4), at(5))
}

fn next_xobject_name(table: &Dict) -> String {
    let mut n = 1u32;
    loop {
        let name = format!("Fm{n}");
        if table.get(&name).is_none() {
            return name;
        }
        n += 1;
    }
}

fn fmt_num(v: f64) -> String {
    if !v.is_finite() || v.abs() < 1e-9 {
        return "0".to_string();
    }
    let rounded = (v * 1_000_000.0).round() / 1_000_000.0;
    let s = format!("{rounded:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
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

    use mangle_content::Resources;
    use mangle_doc::PageTree;
    use mangle_render::{RenderOptions, SsimOptions, compare, render_page};
    use mangle_syntax::{Document, OpenOptions};

    use super::{FlattenError, FlattenFilter, FlattenOptions, flatten_page};

    fn build_annotated_pdf() -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str(
            "1 0 obj\n<</Type /Catalog /Pages 2 0 R /AcroForm <</Fields [8 0 R]>>>>\nendobj\n",
        );
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Contents 4 0 R /Resources 5 0 R /Annots [6 0 R 7 0 R 8 0 R 9 0 R]>>\nendobj\n",
        );
        let body = "0.2 0.2 0.2 rg 10 10 40 40 re f";
        offsets.push(out.len());
        let _ = write!(
            out,
            "4 0 obj\n<</Length {}>>\nstream\n{body}\nendstream\nendobj\n",
            body.len()
        );
        offsets.push(out.len());
        out.push_str("5 0 obj\n<<>>\nendobj\n");
        // 6 0 obj: Square markup annotation with AP 10 0 R
        offsets.push(out.len());
        out.push_str(
            "6 0 obj\n<</Type /Annot /Subtype /Square /Rect [60 60 120 120] \
             /AP <</N 10 0 R>>>>\nendobj\n",
        );
        // 7 0 obj: Link annotation
        offsets.push(out.len());
        out.push_str("7 0 obj\n<</Type /Annot /Subtype /Link /Rect [10 10 50 50]>>\nendobj\n");
        // 8 0 obj: Widget annotation with AP 11 0 R
        offsets.push(out.len());
        out.push_str(
            "8 0 obj\n<</Type /Annot /Subtype /Widget /FT /Tx /T (Name) \
             /Rect [60 130 150 160] /AP <</N 11 0 R>>>>\nendobj\n",
        );
        // 9 0 obj: Hidden annotation (F = 2) with AP 10 0 R
        offsets.push(out.len());
        out.push_str(
            "9 0 obj\n<</Type /Annot /Subtype /Square /F 2 /Rect [130 60 180 120] \
             /AP <</N 10 0 R>>>>\nendobj\n",
        );
        // 10 0 obj: AP stream for Square
        let ap1 = "1 0 0 rg 0 0 60 60 re f";
        offsets.push(out.len());
        let _ = write!(
            out,
            "10 0 obj\n<</Type /XObject /Subtype /Form /BBox [0 0 60 60] /Length {}>>\n\
             stream\n{ap1}\nendstream\nendobj\n",
            ap1.len()
        );
        // 11 0 obj: AP stream for Widget
        let ap2 = "0 0 1 rg 0 0 90 30 re f";
        offsets.push(out.len());
        let _ = write!(
            out,
            "11 0 obj\n<</Type /XObject /Subtype /Form /BBox [0 0 90 30] /Length {}>>\n\
             stream\n{ap2}\nendstream\nendobj\n",
            ap2.len()
        );
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

    fn open_first_page(bytes: Vec<u8>) -> (Document, mangle_doc::Page, Resources) {
        let doc = Document::open(bytes, OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat
            .get("Pages")
            .and_then(mangle_syntax::object::Object::as_ref_id)
            .expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        let page = tree.pages().first().expect("page").clone();
        let res = page
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc.resolve_object(o))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
            .unwrap_or_default();
        (doc, page, res)
    }

    #[test]
    fn flatten_all_is_pixel_equivalent_and_idempotent() {
        let bytes = build_annotated_pdf();
        let (doc_before, page_before, res_before) = open_first_page(bytes);
        let render_before = render_page(
            &doc_before,
            &page_before,
            &res_before,
            RenderOptions::default(),
        );

        let report =
            flatten_page(&doc_before, &page_before, &FlattenOptions::default()).expect("flattens");
        assert_eq!(report.flattened, 3, "Square, Link, Widget flattened");
        assert_eq!(
            report.skipped_hidden, 1,
            "Hidden annotation skipped and counted"
        );

        let (doc_after, page_after, res_after) = open_first_page(report.save.bytes.clone());
        assert_eq!(
            page_after.annots(&doc_after).len(),
            1,
            "only the skipped hidden annotation remains in /Annots"
        );
        let render_after = render_page(
            &doc_after,
            &page_after,
            &res_after,
            RenderOptions::default(),
        );
        let diff = compare(
            &render_before.image,
            &render_after.image,
            &SsimOptions::default(),
        );
        assert!(
            diff.metrics.max_delta <= 1,
            "flatten is pixel-equivalent (max_delta = {}, ssim = {})",
            diff.metrics.max_delta,
            diff.metrics.ssim
        );

        // Second flatten is idempotent and changes nothing.
        let second = flatten_page(&doc_after, &page_after, &FlattenOptions::default())
            .expect("second flatten");
        assert_eq!(second.flattened, 0);
        assert_eq!(second.save.bytes, report.save.bytes);
    }

    #[test]
    fn flatten_markup_only_keeps_links_and_widgets() {
        let bytes = build_annotated_pdf();
        let (doc, page, _) = open_first_page(bytes);
        let report = flatten_page(
            &doc,
            &page,
            &FlattenOptions {
                filter: FlattenFilter::MarkupOnly,
                ..FlattenOptions::default()
            },
        )
        .expect("flattens markup");
        assert_eq!(report.flattened, 1, "only Square markup flattened");
        assert_eq!(report.kept, 2, "Link and Widget kept");
        assert_eq!(report.skipped_hidden, 1);
    }

    #[test]
    fn flatten_forms_only_bakes_widgets_and_drops_empty_acroform() {
        let bytes = build_annotated_pdf();
        let (doc, page, _) = open_first_page(bytes);
        let report = flatten_page(
            &doc,
            &page,
            &FlattenOptions {
                filter: FlattenFilter::FormsOnly,
                ..FlattenOptions::default()
            },
        )
        .expect("flattens forms");
        assert_eq!(report.flattened, 1, "only Widget flattened");
        let (doc_after, _, _) = open_first_page(report.save.bytes);
        let cat = doc_after.catalog().expect("cat");
        assert!(
            cat.as_dict().expect("dict").get("AcroForm").is_none(),
            "empty /AcroForm removed from catalog"
        );
    }

    #[test]
    fn flatten_single_annotation_by_index() {
        let bytes = build_annotated_pdf();
        let (doc, page, _) = open_first_page(bytes);
        let report = flatten_page(
            &doc,
            &page,
            &FlattenOptions {
                filter: FlattenFilter::Indices(vec![0]),
                ..FlattenOptions::default()
            },
        )
        .expect("flattens single annotation");
        assert_eq!(report.flattened, 1);
        assert_eq!(report.kept, 2);
    }

    #[test]
    fn flatten_on_signed_document_warns_and_requires_confirmation() {
        let mut bytes = build_annotated_pdf();
        // Append a dummy signature object with /ByteRange and /Contents.
        let mut update = mangle_syntax::writer::IncrementalUpdate::new(12);
        let mut sig = mangle_syntax::object::Dict::new();
        sig.set("Type", mangle_syntax::object::Object::name("Sig"));
        sig.set(
            "ByteRange",
            mangle_syntax::object::Object::Array(vec![
                mangle_syntax::object::Object::Int(0),
                mangle_syntax::object::Object::Int(10),
                mangle_syntax::object::Object::Int(20),
                mangle_syntax::object::Object::Int(30),
            ]),
        );
        sig.set(
            "Contents",
            mangle_syntax::object::Object::String(b"deadbeef".to_vec()),
        );
        update.set(12, mangle_syntax::object::Object::Dict(sig));
        let start_xref = Document::open(bytes.clone(), OpenOptions::default())
            .expect("opens")
            .xref()
            .start_xref()
            .unwrap_or(0);
        bytes = update.append(
            &bytes,
            start_xref,
            Some(mangle_syntax::object::Ref::new(1, 0)),
        );

        let (doc, page, _) = open_first_page(bytes);
        let err = flatten_page(&doc, &page, &FlattenOptions::default())
            .expect_err("signed document requires confirmation");
        assert_eq!(err, FlattenError::SignedDocumentNeedsConfirmation);

        let report = flatten_page(
            &doc,
            &page,
            &FlattenOptions {
                allow_signed: true,
                ..FlattenOptions::default()
            },
        )
        .expect("confirmed flatten succeeds incrementally");
        assert!(report.signed_warning);
        assert!(report.save.appended_only(doc.bytes()));
    }
}
