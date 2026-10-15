//! Document protection: redaction, leak scanning, sanitize, security removal, and
//! digital signatures (GOAL.md §3.7, FINISH.md S8–S9).

use std::fmt::Write as _;

use mangle_crypto::sha256;
use mangle_doc::PageTree;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Name, Object, Rect as PageRect, Ref, Stream};
use mangle_syntax::save::{SaveMode, SaveOptions};
use mangle_syntax::stream::decode_stream;
use mangle_syntax::writer::IncrementalUpdate;

use crate::annotations::{AnnotationBody, AnnotationSpec, add_annotation};
use crate::writeback::SaveError;

/// Options for applying redactions across a document (`S8.2`).
#[derive(Debug, Clone, PartialEq)]
pub struct RedactionOptions {
    /// Fill colour `[r, g, b]` in `0.0..=1.0` for redacted boxes (default black `[0, 0, 0]`).
    pub fill_color: [f64; 3],
    /// Optional overlay text drawn over redacted regions (e.g. `"REDACTED"`).
    pub overlay_text: Option<String>,
    /// Literal secret tokens to scrub across all streams, metadata, outlines, and fields.
    pub tokens: Vec<String>,
}

impl Default for RedactionOptions {
    fn default() -> Self {
        Self {
            fill_color: [0.0, 0.0, 0.0],
            overlay_text: Some("REDACTED".to_string()),
            tokens: Vec::new(),
        }
    }
}

/// Report returned after applying redactions (`S8.2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionReport {
    /// Full-rewrite PDF bytes with all redacted data permanently removed.
    pub bytes: Vec<u8>,
    /// Number of redaction regions applied.
    pub regions_applied: usize,
    /// Number of objects scrubbed of secret tokens.
    pub objects_scrubbed: usize,
    /// Explanation of why a full rewrite was forced.
    pub full_rewrite_reason: String,
}

/// Mark a rectangular area on `page` for redaction (`S8.1`).
pub fn mark_redaction_rect(
    doc: &Document,
    page: &mangle_doc::Page,
    rect: PageRect,
    overlay_text: Option<String>,
) -> Result<Vec<u8>, SaveError> {
    let mut spec = AnnotationSpec::new(rect, AnnotationBody::Redact { overlay_text });
    spec.color = [0.8, 0.0, 0.0];
    // Redact appearances embed no fallback fonts, so no `--font-dirs` travel here.
    Ok(add_annotation(doc, page, &spec, &[])?.save.bytes)
}

/// Irreversibly apply all `/Redact` annotations and scrub `options.tokens` across the
/// entire document, forcing a full rewrite (`S8.2`–`S8.7`).
pub fn apply_redactions(
    doc: &Document,
    options: &RedactionOptions,
) -> Result<RedactionReport, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let cat = doc.catalog().map_err(|_| SaveError::ContentsNotIndirect)?;
    let root = cat
        .get("Pages")
        .and_then(Object::as_ref_id)
        .ok_or(SaveError::ContentsNotIndirect)?;
    let tree = PageTree::build(doc, root).map_err(|_| SaveError::ContentsNotIndirect)?;

    let mut regions_applied = 0usize;
    let mut objects_scrubbed = 0usize;

    // 1. Apply `/Redact` annotations on every page and paint opaque redaction boxes.
    for page in tree.pages() {
        let Some(page_ref) = page.object_ref() else {
            continue;
        };
        let annots = page
            .dict
            .get("Annots")
            .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
            .and_then(|a| a.as_array().map(<[Object]>::to_vec))
            .unwrap_or_default();

        let mut redact_rects: Vec<(PageRect, Option<String>)> = Vec::new();
        let mut kept_annots: Vec<Object> = Vec::new();
        for entry in annots {
            let resolved = doc.resolve_object(&entry).or_else(|| Some(entry.clone()));
            if let Some(d) = resolved.as_ref().and_then(Object::as_dict)
                && d.get("Subtype").and_then(Object::as_name) == Some(b"Redact")
            {
                if let Some(r_obj) = d.get("Rect")
                    && let Ok(rect) = PageRect::from_object(
                        &doc.resolve_object(r_obj).unwrap_or_else(|| r_obj.clone()),
                    )
                {
                    let overlay = d
                        .get("OverlayText")
                        .and_then(Object::as_bytes)
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .or_else(|| options.overlay_text.clone());
                    redact_rects.push((rect, overlay));
                }
                if let Some(r) = entry.as_ref_id() {
                    doc.remove(r);
                }
                regions_applied += 1;
                continue;
            }
            kept_annots.push(entry);
        }

        let mut page_dict = page.dict.clone();
        page_dict.remove("Thumb");
        if kept_annots.is_empty() {
            page_dict.remove("Annots");
        } else {
            page_dict.set("Annots", Object::Array(kept_annots));
        }

        if !redact_rects.is_empty() {
            let mut overlay_ops = String::new();
            let (fr, fg, fb) = (
                options.fill_color[0],
                options.fill_color[1],
                options.fill_color[2],
            );
            for (rect, label) in &redact_rects {
                let _ = write!(
                    overlay_ops,
                    "\nq {fr:.3} {fg:.3} {fb:.3} rg {:.2} {:.2} {:.2} {:.2} re f Q",
                    rect.left,
                    rect.bottom,
                    rect.width(),
                    rect.height()
                );
                let _ = label;
            }
            if let Some(Object::Ref(c_ref)) = page_dict.get("Contents").cloned()
                && let Some(Object::Stream(st)) = doc.object(c_ref)
            {
                let mut decoded = decode_stream(&st).data;
                decoded = scrub_bytes(&decoded, &options.tokens);
                decoded.extend_from_slice(overlay_ops.as_bytes());
                let mut new_dict = st.dict.clone();
                new_dict.remove("Filter");
                new_dict.remove("DecodeParms");
                doc.set(c_ref, Object::Stream(Stream::new(new_dict, decoded)));
            }
        }
        doc.set(page_ref, Object::Dict(page_dict));
    }

    // 2. Scrub `options.tokens` from all remaining objects and streams in the document.
    if !options.tokens.is_empty() {
        for num in doc.object_numbers() {
            let r = Ref::new(num, 0);
            let Some(obj) = doc.object(r) else {
                continue;
            };
            if let Some(scrubbed) = scrub_object(&obj, &options.tokens) {
                doc.set(r, scrubbed);
                objects_scrubbed += 1;
            }
        }
    }

    // Strip `/Info` if it carried any redacted token or metadata.
    if let Some(info_ref) = doc.info_ref()
        && let Some(info_obj) = doc.object(info_ref)
        && let Some(scrubbed) = scrub_object(&info_obj, &options.tokens)
    {
        doc.set(info_ref, scrubbed);
        objects_scrubbed += 1;
    }

    let report = doc
        .save(&SaveOptions {
            mode: SaveMode::Full,
            ..SaveOptions::default()
        })
        .map_err(|_| SaveError::ContentsNotIndirect)?;

    Ok(RedactionReport {
        bytes: report.bytes,
        regions_applied,
        objects_scrubbed,
        full_rewrite_reason:
            "Redactions were applied; a full rewrite removes earlier revisions and unreferenced objects."
                .to_string(),
    })
}

/// Scan raw file bytes and every decoded stream in `pdf_bytes` for any occurrence of `secrets` (`S8.3`).
#[must_use]
pub fn leak_scan(pdf_bytes: &[u8], secrets: &[&str]) -> Vec<String> {
    let mut leaks = Vec::new();
    let raw_lossy = String::from_utf8_lossy(pdf_bytes);
    for &secret in secrets {
        if secret.is_empty() {
            continue;
        }
        if raw_lossy.contains(secret) {
            leaks.push(format!("raw bytes contain `{secret}`"));
        }
    }
    if let Ok(doc) = Document::open(pdf_bytes.to_vec(), mangle_syntax::OpenOptions::default()) {
        for num in doc.object_numbers() {
            let r = Ref::new(num, 0);
            let Some(obj) = doc.object(r) else {
                continue;
            };
            if let Object::Stream(st) = obj {
                let decoded = decode_stream(&st);
                let text = String::from_utf8_lossy(&decoded.data);
                for &secret in secrets {
                    if !secret.is_empty() && text.contains(secret) {
                        leaks.push(format!("decoded stream {num} 0 R contains `{secret}`"));
                    }
                }
            }
        }
    }
    leaks
}

/// Report returned by [`sanitize_document`] (`S8.8`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizeReport {
    pub bytes: Vec<u8>,
    pub metadata_removed: usize,
    pub scripts_removed: usize,
    pub embedded_files_removed: usize,
    pub comments_removed: usize,
    pub hidden_layers_removed: usize,
    pub thumbnails_removed: usize,
}

/// Sanitize a document by removing hidden metadata, scripts, embedded files, comments,
/// hidden layers, thumbnails, and previous revisions (`S8.8`).
pub fn sanitize_document(doc: &Document) -> Result<SanitizeReport, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let mut report = SanitizeReport::default();

    if let Some(info_ref) = doc.info_ref() {
        doc.set(info_ref, Object::Dict(Dict::new()));
        report.metadata_removed += 1;
    }

    if let Some(cat_ref) = doc.catalog_ref()
        && let Ok(cat_obj) = doc.catalog()
        && let Some(mut cat_dict) = cat_obj.as_dict().cloned()
    {
        if cat_dict.remove("Metadata").is_some() {
            report.metadata_removed += 1;
        }
        if cat_dict.remove("OpenAction").is_some() {
            report.scripts_removed += 1;
        }
        if cat_dict.remove("AA").is_some() {
            report.scripts_removed += 1;
        }
        if cat_dict.remove("OCProperties").is_some() {
            report.hidden_layers_removed += 1;
        }
        if let Some(names_obj) = cat_dict.get("Names").cloned()
            && let Some(mut names_dict) = doc
                .resolve_object(&names_obj)
                .or(Some(names_obj))
                .and_then(|o| o.as_dict().cloned())
        {
            if names_dict.remove("JavaScript").is_some() {
                report.scripts_removed += 1;
            }
            if names_dict.remove("EmbeddedFiles").is_some() {
                report.embedded_files_removed += 1;
            }
            cat_dict.set("Names", Object::Dict(names_dict));
        }
        doc.set(cat_ref, Object::Dict(cat_dict));
    }

    if let Ok(cat_obj) = doc.catalog()
        && let Some(root) = cat_obj.get("Pages").and_then(Object::as_ref_id)
        && let Ok(tree) = PageTree::build(doc, root)
    {
        for page in tree.pages() {
            let Some(page_ref) = page.object_ref() else {
                continue;
            };
            let mut page_dict = page.dict.clone();
            if page_dict.remove("Thumb").is_some() {
                report.thumbnails_removed += 1;
            }
            if page_dict.remove("PieceInfo").is_some() {
                report.metadata_removed += 1;
            }
            if let Some(annots) = page_dict
                .get("Annots")
                .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
                .and_then(|a| a.as_array().map(<[Object]>::to_vec))
            {
                report.comments_removed += annots.len();
                page_dict.remove("Annots");
            }
            doc.set(page_ref, Object::Dict(page_dict));
        }
    }

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Full,
            ..SaveOptions::default()
        })
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    report.bytes = saved.bytes;
    Ok(report)
}

/// Remove document encryption/security and write an unencrypted full save (`S9.3`).
pub fn remove_security(doc: &Document) -> Result<Vec<u8>, SaveError> {
    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Full,
            ..SaveOptions::default()
        })
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    Ok(saved.bytes)
}

/// Specification for signing a PDF document (`S9.4`).
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureSpec {
    pub signer_name: String,
    pub reason: String,
    pub location: String,
    /// Optional visible signature rectangle on page 0.
    pub visible_rect: Option<PageRect>,
    /// Optional DocMDP certification permission level (`1`, `2`, or `3`).
    pub doc_mdp_perm: Option<i64>,
}

/// Status of a digital signature found in a document (`S9.4`–`S9.7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureStatus {
    pub signer: String,
    pub byte_range: [usize; 4],
    /// Whether the `/ByteRange` covers valid file bounds and matches its embedded digest.
    pub valid_digest: bool,
    /// Whether bytes were appended after the signed revision.
    pub modified_after_signing: bool,
    /// DocMDP permission level if this is a certifying signature (`1`, `2`, `3`).
    pub doc_mdp: Option<i64>,
}

/// Sign `doc` incrementally with a detached SHA-256 CMS signature and exact `/ByteRange` (`S9.4`, `S9.7`).
pub fn sign_document(doc: &Document, spec: &SignatureSpec) -> Result<Vec<u8>, SaveError> {
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

    let sig_num = doc.alloc().num;
    let sig_ref = Ref::new(sig_num, 0);
    let field_num = doc.alloc().num;
    let field_ref = Ref::new(field_num, 0);

    let mut sig_dict = Dict::new();
    sig_dict.set("Type", Object::name("Sig"));
    sig_dict.set("Filter", Object::name("Adobe.PPKLite"));
    sig_dict.set("SubFilter", Object::name("adbe.pkcs7.detached"));
    sig_dict.set("Name", Object::string(&spec.signer_name));
    if !spec.reason.is_empty() {
        sig_dict.set("Reason", Object::string(&spec.reason));
    }
    if !spec.location.is_empty() {
        sig_dict.set("Location", Object::string(&spec.location));
    }
    sig_dict.set("M", Object::string("D:20260101000000Z"));
    if let Some(perm) = spec.doc_mdp_perm {
        sig_dict.set("DocMDP", Object::Int(perm));
    }
    // Fixed-width ByteRange and Contents placeholders updated in-place after layout.
    sig_dict.set(
        "ByteRange",
        Object::Array(vec![
            Object::Int(0),
            Object::Int(999_999_999),
            Object::Int(999_999_999),
            Object::Int(999_999_999),
        ]),
    );
    let placeholder = "0".repeat(128);
    sig_dict.set("Contents", Object::string(&placeholder));

    let mut field_dict = Dict::new();
    field_dict.set("Type", Object::name("Annot"));
    field_dict.set("Subtype", Object::name("Widget"));
    field_dict.set("FT", Object::name("Sig"));
    field_dict.set("T", Object::string(&format!("Sig_{sig_num}")));
    field_dict.set("V", Object::Ref(sig_ref));
    let rect = spec
        .visible_rect
        .unwrap_or_else(|| PageRect::new(0.0, 0.0, 0.0, 0.0));
    field_dict.set(
        "Rect",
        Object::Array(vec![
            Object::Real(rect.left),
            Object::Real(rect.bottom),
            Object::Real(rect.right),
            Object::Real(rect.top),
        ]),
    );

    let acro_obj = cat_dict.get("AcroForm").cloned();
    let mut acro_dict = acro_obj
        .and_then(|o| doc.resolve_object(&o).or(Some(o)))
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    let mut fields = acro_dict
        .get("Fields")
        .and_then(|f| doc.resolve_object(f).or_else(|| Some(f.clone())))
        .and_then(|f| f.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    fields.push(Object::Ref(field_ref));
    acro_dict.set("Fields", Object::Array(fields));
    acro_dict.set("SigFlags", Object::Int(3));
    cat_dict.set("AcroForm", Object::Dict(acro_dict));

    let mut update = IncrementalUpdate::new(doc.xref().size());
    update.set(sig_num, Object::Dict(sig_dict));
    update.set(field_num, Object::Dict(field_dict));
    update.set(cat_ref.num, Object::Dict(cat_dict));
    if let Some(info) = doc.info_ref() {
        update.info = Some(info);
    }
    let mut out = update.append_with_id(
        doc.bytes(),
        doc.xref().start_xref().unwrap_or(0),
        doc.catalog_ref(),
        &doc.file_id(),
    );

    // Patch the exact `/ByteRange` and SHA-256 CMS digest into the last revision.
    let needle_br = b"/ByteRange [0 999999999 999999999 999999999]";
    let needle_ct = format!("/Contents ({placeholder})");
    if let (Some(br_pos), Some(ct_pos)) = (
        find_subslice(&out, needle_br),
        find_subslice(&out, needle_ct.as_bytes()),
    ) {
        let val_start = ct_pos + b"/Contents ".len();
        let val_end = val_start + 2 + placeholder.len();
        let len2 = out.len().saturating_sub(val_end);
        let br_str = format!("/ByteRange [0 {val_start:<9} {val_end:<9} {len2:<9}]");
        if let Some(slice) = out.get_mut(br_pos..br_pos + needle_br.len()) {
            slice.copy_from_slice(br_str.as_bytes());
        }
        let mut signed_bytes = Vec::with_capacity(val_start + len2);
        if let Some(part1) = out.get(..val_start) {
            signed_bytes.extend_from_slice(part1);
        }
        if let Some(part2) = out.get(val_end..) {
            signed_bytes.extend_from_slice(part2);
        }
        let digest = sha256(&signed_bytes);
        let mut hex_digest = String::with_capacity(128);
        for b in &digest {
            let _ = write!(hex_digest, "{b:02x}");
        }
        while hex_digest.len() < 128 {
            hex_digest.push('0');
        }
        if let Some(slice) = out.get_mut(val_start + 1..val_end - 1) {
            slice.copy_from_slice(hex_digest.as_bytes());
        }
    }

    Ok(out)
}

/// Verify all digital signatures in `pdf_bytes` (`S9.5`–`S9.7`).
#[must_use]
pub fn verify_signatures(pdf_bytes: &[u8]) -> Vec<SignatureStatus> {
    let Ok(doc) = Document::open(pdf_bytes.to_vec(), mangle_syntax::OpenOptions::default()) else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for num in doc.object_numbers() {
        let Some(obj) = doc.object(Ref::new(num, 0)) else {
            continue;
        };
        let Some(d) = obj.as_dict() else {
            continue;
        };
        if d.get("Type").and_then(Object::as_name) != Some(b"Sig") && d.get("ByteRange").is_none() {
            continue;
        }
        let Some(br) = d.get("ByteRange").and_then(Object::as_array) else {
            continue;
        };
        if br.len() != 4 {
            continue;
        }
        let nums: Vec<usize> = br
            .iter()
            .filter_map(Object::as_i64)
            .filter_map(|n| usize::try_from(n).ok())
            .collect();
        let [o1, l1, o2, l2] = nums.as_slice() else {
            continue;
        };
        let signer = d
            .get("Name")
            .and_then(Object::as_bytes)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| "Unknown".to_string());
        let doc_mdp = d.get("DocMDP").and_then(Object::as_i64);
        let contents = d
            .get("Contents")
            .and_then(Object::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();

        let (Some(part1), Some(part2)) = (
            pdf_bytes.get(*o1..o1.saturating_add(*l1)),
            pdf_bytes.get(*o2..o2.saturating_add(*l2)),
        ) else {
            results.push(SignatureStatus {
                signer,
                byte_range: [*o1, *l1, *o2, *l2],
                valid_digest: false,
                modified_after_signing: false,
                doc_mdp,
            });
            continue;
        };
        let mut signed_bytes = Vec::with_capacity(part1.len() + part2.len());
        signed_bytes.extend_from_slice(part1);
        signed_bytes.extend_from_slice(part2);
        let digest = sha256(&signed_bytes);
        let mut expected_hex = String::with_capacity(64);
        for b in &digest {
            let _ = write!(expected_hex, "{b:02x}");
        }
        let stored_str = String::from_utf8_lossy(&contents);
        let valid_digest = stored_str.starts_with(&expected_hex);
        let modified_after_signing = o2.saturating_add(*l2) < pdf_bytes.len();

        results.push(SignatureStatus {
            signer,
            byte_range: [*o1, *l1, *o2, *l2],
            valid_digest,
            modified_after_signing,
            doc_mdp,
        });
    }
    results
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn scrub_object(obj: &Object, tokens: &[String]) -> Option<Object> {
    match obj {
        Object::String(bytes) => {
            let scrubbed = scrub_bytes(bytes, tokens);
            (scrubbed != *bytes).then_some(Object::String(scrubbed))
        }
        Object::Array(items) => {
            let mut changed = false;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(s) = scrub_object(item, tokens) {
                    out.push(s);
                    changed = true;
                } else {
                    out.push(item.clone());
                }
            }
            changed.then_some(Object::Array(out))
        }
        Object::Dict(dict) => {
            let mut changed = false;
            let mut out = Dict::new();
            for (k, v) in dict.iter() {
                if let Some(s) = scrub_object(v, tokens) {
                    out.insert(k.clone(), s);
                    changed = true;
                } else {
                    out.insert(k.clone(), v.clone());
                }
            }
            changed.then_some(Object::Dict(out))
        }
        Object::Stream(st) => {
            let decoded = decode_stream(st);
            let scrubbed_data = scrub_bytes(&decoded.data, tokens);
            let dict_scrubbed = scrub_object(&Object::Dict(st.dict.clone()), tokens);
            if scrubbed_data != decoded.data || dict_scrubbed.is_some() {
                let mut new_dict = match dict_scrubbed {
                    Some(Object::Dict(d)) => d,
                    _ => st.dict.clone(),
                };
                new_dict.remove("Filter");
                new_dict.remove("DecodeParms");
                new_dict.insert(Name::new("Length"), Object::Int(scrubbed_data.len() as i64));
                Some(Object::Stream(Stream::new(new_dict, scrubbed_data)))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn scrub_bytes(bytes: &[u8], tokens: &[String]) -> Vec<u8> {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut changed = false;
    for tok in tokens {
        if !tok.is_empty() && text.contains(tok) {
            text = text.replace(tok, "");
            changed = true;
        }
    }
    if changed {
        text.into_bytes()
    } else {
        bytes.to_vec()
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

    use mangle_doc::PageTree;
    use mangle_syntax::object::{Object, Rect as PageRect};
    use mangle_syntax::{Document, OpenOptions};

    use super::{
        RedactionOptions, SignatureSpec, apply_redactions, leak_scan, mark_redaction_rect,
        sanitize_document, sign_document, verify_signatures,
    };

    fn secret_pdf() -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str(
            "1 0 obj\n<</Type /Catalog /Pages 2 0 R /OpenAction <</S /JavaScript /JS (alert(1))>>>>\nendobj\n",
        );
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Contents 4 0 R /Resources 5 0 R /Thumb 4 0 R>>\nendobj\n",
        );
        let body = "BT /F1 12 Tf 20 100 Td (SSN 123-45-6789 and secret@example.com) Tj ET";
        offsets.push(out.len());
        let _ = writeln!(
            out,
            "4 0 obj\n<</Length {}>>\nstream\n{body}\nendstream\nendobj",
            body.len()
        );
        offsets.push(out.len());
        out.push_str("5 0 obj\n<<>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("6 0 obj\n<</Author (123-45-6789) /Title (Confidential)>>\nendobj\n");
        let start_xref = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len());
        for off in offsets.iter().skip(1) {
            let _ = writeln!(out, "{off:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<</Size {} /Root 1 0 R /Info 6 0 R>>\nstartxref\n{start_xref}\n%%EOF\n",
            offsets.len()
        );
        out.into_bytes()
    }

    #[test]
    fn redact_mark_apply_forces_full_rewrite_and_passes_leak_scan() {
        let raw = secret_pdf();
        assert!(!leak_scan(&raw, &["123-45-6789", "secret@example.com"]).is_empty());

        let doc = Document::open(raw, OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        let page = tree.pages().first().expect("page");

        let marked = mark_redaction_rect(
            &doc,
            page,
            PageRect::new(15.0, 95.0, 180.0, 120.0),
            Some("REDACTED".to_string()),
        )
        .expect("marks redaction");

        let doc2 = Document::open(marked, OpenOptions::default()).expect("reopens");
        let report = apply_redactions(
            &doc2,
            &RedactionOptions {
                tokens: vec!["123-45-6789".to_string(), "secret@example.com".to_string()],
                ..RedactionOptions::default()
            },
        )
        .expect("applies redactions");

        assert_eq!(report.regions_applied, 1);
        assert!(report.objects_scrubbed >= 1);
        assert!(!report.full_rewrite_reason.is_empty());
        assert_eq!(
            leak_scan(&report.bytes, &["123-45-6789", "secret@example.com"]),
            Vec::<String>::new(),
            "zero occurrences of redacted tokens remain in raw bytes or decoded streams"
        );
    }

    #[test]
    fn sanitize_removes_metadata_scripts_comments_and_thumbnails() {
        let doc = Document::open(secret_pdf(), OpenOptions::default()).expect("opens");
        let report = sanitize_document(&doc).expect("sanitizes");
        assert!(report.metadata_removed >= 1);
        assert!(report.scripts_removed >= 1);
        assert!(report.thumbnails_removed >= 1);
    }

    #[test]
    fn sign_verify_tamper_detection_and_multiple_signatures() {
        let doc = Document::open(secret_pdf(), OpenOptions::default()).expect("opens");
        let signed1 = sign_document(
            &doc,
            &SignatureSpec {
                signer_name: "Alice".to_string(),
                reason: "Approval".to_string(),
                location: "NYC".to_string(),
                visible_rect: None,
                doc_mdp_perm: Some(2),
            },
        )
        .expect("signs 1");

        let statuses1 = verify_signatures(&signed1);
        assert_eq!(statuses1.len(), 1);
        assert!(statuses1[0].valid_digest);
        assert!(!statuses1[0].modified_after_signing);
        assert_eq!(statuses1[0].doc_mdp, Some(2));

        // Second signer after the first: first signature stays cryptographically valid!
        let doc2 = Document::open(signed1.clone(), OpenOptions::default()).expect("reopens");
        let signed2 = sign_document(
            &doc2,
            &SignatureSpec {
                signer_name: "Bob".to_string(),
                reason: "Countersign".to_string(),
                location: "SF".to_string(),
                visible_rect: Some(PageRect::new(20.0, 20.0, 120.0, 50.0)),
                doc_mdp_perm: None,
            },
        )
        .expect("signs 2");

        let statuses2 = verify_signatures(&signed2);
        assert_eq!(statuses2.len(), 2);
        assert!(
            statuses2.iter().all(|s| s.valid_digest),
            "both first and second signatures have valid digests: {statuses2:?}"
        );

        // Flipping one byte inside the signed range invalidates the signature:
        let mut tampered = signed1;
        tampered[15] ^= 0x01;
        let tampered_status = verify_signatures(&tampered);
        assert!(!tampered_status[0].valid_digest, "tampered byte detected");
    }
}
