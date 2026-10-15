//! Paragraph reflow, alignment (`Left`, `Center`, `Right`, `Justify`), and shared Form XObject
//! copy-on-write (GOAL.md §4.3–4.4, FINISH.md S1.9, S1.15).

use std::fmt::Write as _;

use mangle_doc::{Page, PageTree};
use mangle_font::{Encoding as FontEncoding, EncodingBase as FontBase};
use mangle_syntax::document::Document;
use mangle_syntax::object::{Name, Object, Ref, Stream};
use mangle_syntax::stream::{decode_stream, encode_stream};
use mangle_syntax::writer::IncrementalUpdate;

use crate::page_objects::PageObject;
use crate::surgery::{Patch, apply};
use crate::writeback::{Save, SaveError};

/// Horizontal alignment of a text block (`GOAL.md` §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlignment {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// One laid-out line produced by [`layout_paragraph`].
#[derive(Debug, Clone, PartialEq)]
pub struct LaidOutLine {
    /// Words on this line joined by single ASCII spaces.
    pub text: String,
    /// Horizontal offset from the block's left edge, in points.
    pub x_offset: f64,
    /// Baseline y coordinate in page space.
    pub y: f64,
    /// Word spacing (`Tw`) in unscaled text-space points (`0.0` unless justified).
    pub word_spacing: f64,
    /// Total rendered width of this line (including `word_spacing`), in points.
    pub rendered_width: f64,
}

/// Outcome of laying out and reflowing a paragraph (`S1.9`).
#[derive(Debug, Clone, PartialEq)]
pub struct ReflowLayout {
    /// Laid-out lines from top to bottom.
    pub lines: Vec<LaidOutLine>,
    /// Whether the total height of the reflowed lines exceeded the original block height.
    pub overflow: bool,
    /// Notice message when `overflow` is `true`.
    pub overflow_notice: Option<String>,
}

/// Advance width of `word` in points for `base_font` at `font_size`, `char_spacing` (`Tc`),
/// and `horizontal_scale` (`Tz` in percent).
#[must_use]
pub fn measure_word(
    word: &str,
    base_font: &str,
    font_size: f64,
    char_spacing: f64,
    horizontal_scale: f64,
) -> f64 {
    let table = mangle_font::widths(base_font).or_else(|| mangle_font::widths("Helvetica"));
    let scale = horizontal_scale / 100.0;
    let mut total = 0.0;
    let count = word.chars().count();
    for (idx, ch) in word.chars().enumerate() {
        let glyph_w = f64::from(
            table
                .and_then(|w| {
                    let code = u32::from(ch);
                    mangle_font::agl_name(code).and_then(|name| mangle_font::by_name(w, name))
                })
                .unwrap_or(500),
        );
        total += (glyph_w / 1000.0 * font_size) * scale;
        if idx + 1 < count {
            total += char_spacing * scale;
        }
    }
    total
}

/// Space glyph width in points for `base_font` at `font_size`, `char_spacing`, and `horizontal_scale`.
#[must_use]
pub fn measure_space(
    base_font: &str,
    font_size: f64,
    char_spacing: f64,
    horizontal_scale: f64,
) -> f64 {
    let table = mangle_font::widths(base_font).or_else(|| mangle_font::widths("Helvetica"));
    let w = f64::from(
        table
            .and_then(|t| mangle_font::by_name(t, "space"))
            .unwrap_or(278),
    );
    let scale = horizontal_scale / 100.0;
    (w / 1000.0 * font_size + char_spacing) * scale
}

/// Lay out `text` inside a column of `width` and `max_height` points starting at `(x0, top_y)` (`S1.9`).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn layout_paragraph(
    text: &str,
    base_font: &str,
    font_size: f64,
    leading: f64,
    char_spacing: f64,
    horizontal_scale: f64,
    x0: f64,
    top_y: f64,
    width: f64,
    max_height: f64,
    alignment: TextAlignment,
) -> ReflowLayout {
    let _ = x0;
    let col_w = width.max(font_size.max(1.0));
    let line_step = if leading > 0.0 {
        leading
    } else {
        font_size * 1.2
    };
    let space_w = measure_space(base_font, font_size, char_spacing, horizontal_scale);
    let scale = (horizontal_scale / 100.0).max(0.01);

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut raw_lines: Vec<(Vec<&str>, f64)> = Vec::new();
    let mut cur_words: Vec<&str> = Vec::new();
    let mut cur_w = 0.0;

    for w in words {
        let ww = measure_word(w, base_font, font_size, char_spacing, horizontal_scale);
        if cur_words.is_empty() {
            cur_words.push(w);
            cur_w = ww;
        } else if cur_w + space_w + ww <= col_w + 1e-6 {
            cur_words.push(w);
            cur_w += space_w + ww;
        } else {
            raw_lines.push((cur_words, cur_w));
            cur_words = vec![w];
            cur_w = ww;
        }
    }
    if !cur_words.is_empty() {
        raw_lines.push((cur_words, cur_w));
    }

    let total_lines = raw_lines.len();
    let mut lines = Vec::with_capacity(total_lines);
    for (idx, (line_words, natural_w)) in raw_lines.into_iter().enumerate() {
        let is_last = idx + 1 == total_lines;
        let gaps = line_words.len().saturating_sub(1);
        let (x_offset, word_spacing, rendered_width) = match alignment {
            TextAlignment::Left => (0.0, 0.0, natural_w),
            TextAlignment::Center => (((col_w - natural_w) * 0.5).max(0.0), 0.0, natural_w),
            TextAlignment::Right => ((col_w - natural_w).max(0.0), 0.0, natural_w),
            TextAlignment::Justify => {
                if !is_last && gaps > 0 && natural_w < col_w {
                    let extra_per_gap = (col_w - natural_w) / (gaps as f64);
                    let tw = extra_per_gap / scale;
                    (0.0, tw, col_w)
                } else {
                    (0.0, 0.0, natural_w)
                }
            }
        };
        let y = top_y - (idx as f64) * line_step;
        lines.push(LaidOutLine {
            text: line_words.join(" "),
            x_offset,
            y,
            word_spacing,
            rendered_width,
        });
    }

    let needed_height = if lines.len() <= 1 {
        font_size
    } else {
        ((lines.len() - 1) as f64) * line_step + font_size
    };
    let overflow = max_height > 0.0 && needed_height > max_height + 0.5;
    let overflow_notice = overflow.then(|| {
        format!(
            "reflowed paragraph height ({needed_height:.1} pt) exceeds block height ({max_height:.1} pt)"
        )
    });

    ReflowLayout {
        lines,
        overflow,
        overflow_notice,
    }
}

/// The measurements a reflow lays out from: the font resource and size, the leading
/// and spacings, and the block's left edge, top baseline, width, and height.
///
/// Factored out of [`reflow_patches`] so the verifier recomputes the same layout
/// from the same numbers rather than trusting remembered ones.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReflowInputs {
    /// The font resource, as the stream named it.
    pub font: String,
    /// The type size in points.
    pub size: f64,
    /// The leading (`TL`, or 1.2× size).
    pub leading: f64,
    /// Character spacing (`Tc`).
    pub char_spacing: f64,
    /// Horizontal scale (`Tz`, percent).
    pub horizontal_scale: f64,
    /// The block's left edge in page space.
    pub x0: f64,
    /// The first baseline in page space.
    pub top_y: f64,
    /// The block's width in points.
    pub width: f64,
    /// The block's height in points, for the overflow flag.
    pub max_height: f64,
}

/// Read the measurements [`reflow_patches`] lays out from.
///
/// `None` when the object has no records, which is also what refuses the edit.
#[must_use]
pub(crate) fn reflow_inputs(object: &PageObject) -> Option<ReflowInputs> {
    let first_rec = object.records.first()?;
    let size = first_rec.text.size.max(1.0);
    Some(ReflowInputs {
        font: first_rec
            .text
            .font
            .clone()
            .unwrap_or_else(|| "F1".to_string()),
        size,
        leading: if first_rec.text.leading > 0.0 {
            first_rec.text.leading
        } else {
            size * 1.2
        },
        char_spacing: first_rec.text.char_spacing,
        horizontal_scale: first_rec.text.horizontal_scale,
        x0: object.bounds.x0,
        top_y: (object.bounds.y1 - size * 0.75).max(object.bounds.y0),
        width: (object.bounds.x1 - object.bounds.x0).max(size * 4.0),
        max_height: (object.bounds.y1 - object.bounds.y0).max(size),
    })
}

/// Build surgical byte-range patches that replace `object` with `text` reflowed inside
/// `object`'s original width and aligned according to `alignment` (`S1.9`).
///
/// The replacement stays inside the object's existing `BT`/`ET` (one `q`…`Q` wrapper)
/// and encodes lines in WinAnsi; non-WinAnsi text is refused so it can go through the
/// subset-embedding retype path instead of rendering as mojibake.
pub fn reflow_patches(
    object: &PageObject,
    text: &str,
    base_font: &str,
    alignment: TextAlignment,
) -> Result<(Vec<Patch>, ReflowLayout), String> {
    let Some(first_span) = object.spans.first() else {
        return Err("object has no spans to reflow".to_string());
    };
    let Some(inputs) = reflow_inputs(object) else {
        return Err("object has no records to reflow".to_string());
    };

    let layout = layout_paragraph(
        text,
        base_font,
        inputs.size,
        inputs.leading,
        inputs.char_spacing,
        inputs.horizontal_scale,
        inputs.x0,
        inputs.top_y,
        inputs.width,
        inputs.max_height,
        alignment,
    );

    let mut ops = format!("q /{} {} Tf ", inputs.font, fmt_num(inputs.size));
    if inputs.char_spacing.abs() > 1e-6 {
        let _ = write!(ops, "{} Tc ", fmt_num(inputs.char_spacing));
    }
    if (inputs.horizontal_scale - 100.0).abs() > 1e-6 {
        let _ = write!(ops, "{} Tz ", fmt_num(inputs.horizontal_scale));
    }
    for line in &layout.lines {
        let encoded = encode_winansi_line(&line.text).map_err(|ch| {
            format!("character {ch:?} is not in WinAnsi and needs a subset embedding instead")
        })?;
        let mut lit = Vec::new();
        mangle_syntax::writer::write_literal_string(&mut lit, &encoded);
        let lit_str = String::from_utf8_lossy(&lit);
        let _ = write!(
            ops,
            "1 0 0 1 {} {} Tm {} Tw {lit_str} Tj ",
            fmt_num(inputs.x0 + line.x_offset),
            fmt_num(line.y),
            fmt_num(line.word_spacing),
        );
    }
    ops.push_str("0 Tw Q");

    let mut patches = Vec::with_capacity(object.spans.len());
    patches.push(Patch {
        range: first_span.clone(),
        bytes: ops.into_bytes(),
        label: format!("reflow paragraph ({alignment:?})"),
    });
    for span in object.spans.iter().skip(1) {
        patches.push(Patch {
            range: span.clone(),
            bytes: b" ".to_vec(),
            label: "remove old line in reflowed block".to_string(),
        });
    }
    Ok((patches, layout))
}

// ---- Shared Form XObject copy-on-write vs edit-everywhere (S1.15) ----

/// Scope of an edit to a Form XObject that is referenced on multiple pages (`S1.15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedFormScope {
    /// Copy-on-write: clone the Form XObject for `this_page` only, leaving other pages byte-identical.
    ThisPageOnly,
    /// Edit the shared Form XObject in place so every page referencing it updates.
    Everywhere,
}

/// Information about a Form XObject's sharing across the document (`S1.15`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFormInfo {
    /// Object reference of the Form XObject stream.
    pub form_ref: Ref,
    /// Indices of all pages (0-based) that reference this Form XObject.
    pub pages_using: Vec<usize>,
    /// User-facing prompt when `pages_using.len() > 1`, e.g. `"Form XObject /Fm0 is used on 2 pages"`.
    pub prompt: Option<String>,
}

/// Inspect how many pages in `doc` reference the Form XObject named `form_name` on `page` (`S1.15`).
pub fn inspect_shared_form(
    doc: &Document,
    page: &Page,
    form_name: &str,
) -> Result<SharedFormInfo, SaveError> {
    let form_ref = resolve_form_ref(doc, page, form_name).ok_or(SaveError::ContentsNotIndirect)?;
    let cat = doc.catalog().map_err(|_| SaveError::ContentsNotIndirect)?;
    let root = cat
        .get("Pages")
        .and_then(Object::as_ref_id)
        .ok_or(SaveError::ContentsNotIndirect)?;
    let tree = PageTree::build(doc, root).map_err(|_| SaveError::ContentsNotIndirect)?;

    let mut pages_using = Vec::new();
    for (idx, p) in tree.pages().iter().enumerate() {
        let Some(res) = p
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
        else {
            continue;
        };
        let Some(xobjs) = res
            .get("XObject")
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
        else {
            continue;
        };
        let uses_form = xobjs.iter().any(|(_, v)| v.as_ref_id() == Some(form_ref));
        if uses_form {
            pages_using.push(idx);
        }
    }

    let prompt = (pages_using.len() > 1).then(|| {
        format!(
            "Form XObject /{form_name} is used on {} pages; edit on this page only or everywhere?",
            pages_using.len()
        )
    });

    Ok(SharedFormInfo {
        form_ref,
        pages_using,
        prompt,
    })
}

/// Apply `patches` to the Form XObject `form_name` on `page` with the chosen `scope` (`S1.15`).
pub fn edit_shared_form(
    doc: &Document,
    page: &Page,
    form_name: &str,
    patches: &[Patch],
    scope: SharedFormScope,
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let form_ref = resolve_form_ref(doc, page, form_name).ok_or(SaveError::ContentsNotIndirect)?;
    let Some(Object::Stream(form_stream)) = doc.object(form_ref) else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let decoded = decode_stream(&form_stream);
    let filters = form_stream.filters();
    if filters.len() > 1 {
        return Err(SaveError::FilterChain {
            filters: filters
                .iter()
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect(),
        });
    }
    if let Some(first) = filters.first()
        && mangle_syntax::stream::is_terminating_filter(first)
    {
        return Err(SaveError::FilterChain {
            filters: filters
                .iter()
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect(),
        });
    }
    if !decoded.notes.is_empty() && decoded.data.is_empty() {
        return Err(SaveError::Undecodable {
            part: 0,
            note: decoded.notes.join("; "),
        });
    }
    let applied = apply(&decoded.data, patches)?;

    let params = form_stream.decode_parms().into_iter().next().flatten();
    let (raw, written_parms) = match filters.first() {
        Some(f) => encode_stream(&applied.bytes, f, params),
        None => (applied.bytes, None),
    };
    let mut new_dict = mangle_syntax::object::Dict::new();
    for (key, value) in form_stream.dict.iter() {
        if matches!(
            key.as_bytes(),
            b"Length" | b"Filter" | b"DecodeParms" | b"DP"
        ) {
            continue;
        }
        new_dict.insert(key.clone(), value.clone());
    }
    if let Some(filter) = filters.first() {
        new_dict.set("Filter", Object::Name(Name::from_bytes(filter)));
    }
    if let Some(p) = written_parms {
        new_dict.set("DecodeParms", p);
    }
    let new_stream = Stream::new(new_dict, raw);

    let mut extra = Vec::new();
    match scope {
        SharedFormScope::Everywhere => {
            extra.push((form_ref.num, Object::Stream(new_stream)));
        }
        SharedFormScope::ThisPageOnly => {
            let page_ref = page.object_ref().ok_or(SaveError::ContentsNotIndirect)?;
            let copy_num = doc.alloc().num;
            let copy_ref = Ref::new(copy_num, 0);
            extra.push((copy_num, Object::Stream(new_stream)));

            let mut res_dict = page
                .inherited
                .resources
                .as_ref()
                .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
                .and_then(|o| o.as_dict().cloned())
                .unwrap_or_default();
            let mut xobjs = res_dict
                .get("XObject")
                .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
                .and_then(|o| o.as_dict().cloned())
                .unwrap_or_default();
            xobjs.insert(Name::new(form_name), Object::Ref(copy_ref));
            res_dict.set("XObject", Object::Dict(xobjs));

            let res_num = doc.alloc().num;
            extra.push((res_num, Object::Dict(res_dict)));
            let mut page_dict = page.dict.clone();
            page_dict.set("Resources", Object::Ref(Ref::new(res_num, 0)));
            extra.push((page_ref.num, Object::Dict(page_dict)));
        }
    }

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
    Ok(Save {
        bytes,
        rewritten: match scope {
            SharedFormScope::Everywhere => vec![form_ref.num],
            SharedFormScope::ThisPageOnly => Vec::new(),
        },
        installed: extra.iter().map(|(n, _)| *n).collect(),
        re_encoded: !filters.is_empty(),
    })
}

fn resolve_form_ref(doc: &Document, page: &Page, form_name: &str) -> Option<Ref> {
    let res = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())?;
    let xobjs = res
        .get("XObject")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())?;
    xobjs.get(form_name)?.as_ref_id()
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

/// Encode one reflowed line in WinAnsi, refusing honestly when it cannot be done.
///
/// Reflow writes into the run's own simple font, so the bytes must be codes in that
/// font's encoding. This helper assumes the common WinAnsi base and refuses glyphs
/// outside it rather than emitting UTF-8 bytes that would render as mojibake. Unicode
/// reflow needs a subset embedding via the rung-3 retype path instead. The refusal
/// names the character so a caller can name it back rather than rewording it.
pub(crate) fn encode_winansi_line(line: &str) -> Result<Vec<u8>, char> {
    let encoding = FontEncoding::new(FontBase::WinAnsi);
    let mut out = Vec::with_capacity(line.len());
    for ch in line.chars() {
        let code_point = u32::from(ch);
        let name = mangle_font::agl_name(code_point).ok_or(ch)?;
        let code = if code_point <= 255 && encoding.glyph_for(code_point) == Some(name) {
            code_point
        } else {
            encoding.code_for(name).ok_or(ch)?
        };
        if code > 255 {
            return Err(ch);
        }
        out.push(code as u8);
    }
    Ok(out)
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

    use mangle_content::{ContentStream, Resources, interp};
    use mangle_doc::PageTree;
    use mangle_syntax::object::Object;
    use mangle_syntax::{Document, OpenOptions};

    use crate::page_objects::{PageModel, page_text};
    use crate::surgery::{Patch, apply};

    use super::{
        SharedFormScope, TextAlignment, edit_shared_form, inspect_shared_form, layout_paragraph,
        reflow_patches,
    };

    #[test]
    fn sixty_word_reflow_justifies_edges_and_keeps_last_line_left_aligned() {
        let words = "\
            Explore ancient coastal villages perched above sapphire waters where cobbled \
            lanes wind past whitewashed chapels and terraced olive groves. Travellers who \
            linger past sunset discover quiet harbour taverns serving grilled octopus, local \
            citrus, and crisp island wine while fishing boats rock gently under lantern light \
            along the old stone quay and distant bells echo across the calm bay at dusk.";
        assert!(words.split_whitespace().count() >= 55);

        let col_w = 220.0;
        let layout = layout_paragraph(
            words,
            "Helvetica",
            10.0,
            13.0,
            0.0,
            100.0,
            50.0,
            700.0,
            col_w,
            60.0,
            TextAlignment::Justify,
        );
        assert!(layout.lines.len() >= 6);
        assert!(
            layout.overflow,
            "60 words in a 60pt tall box flags overflow"
        );
        assert!(layout.overflow_notice.is_some());

        // Every non-last line has justified rendered width within 0.5 pt of `col_w`!
        for line in layout.lines.iter().take(layout.lines.len() - 1) {
            assert!(
                (line.rendered_width - col_w).abs() <= 0.5,
                "justified line width {} not within 0.5 pt of {col_w}",
                line.rendered_width
            );
            assert!(line.word_spacing > 0.0);
        }
        // Last line is left-aligned (`x_offset == 0.0`, `word_spacing == 0.0`):
        let last = layout.lines.last().unwrap();
        assert_eq!(last.x_offset, 0.0);
        assert_eq!(last.word_spacing, 0.0);
    }

    #[test]
    fn surgical_reflow_leaves_neighbouring_column_untouched() {
        let stream = b"BT /F1 10 Tf 1 0 0 1 40 700 Tm (First column text here) Tj ET \
                       BT /F1 10 Tf 1 0 0 1 300 650 Tm (Second column stays identical) Tj ET";
        let mut font = mangle_syntax::object::Dict::new();
        font.set("Type", Object::name("Font"));
        font.set("Subtype", Object::name("Type1"));
        font.set("BaseFont", Object::name("Helvetica"));
        font.set("Encoding", Object::name("WinAnsiEncoding"));
        let mut fonts = mangle_syntax::object::Dict::new();
        fonts.set("F1", Object::Dict(font));
        let mut res_dict = mangle_syntax::object::Dict::new();
        res_dict.set("Font", Object::Dict(fonts));
        let resources = Resources::from_dict(&res_dict, &|_| None);

        let before = interp::run_with(&ContentStream::parse(stream), &resources);
        let model = PageModel::build(&before.records);
        assert_eq!(model.objects().len(), 2);

        let (patches, _) = reflow_patches(
            &model.objects()[0],
            "Updated first column with several words that wrap cleanly inside the column",
            "Helvetica",
            TextAlignment::Justify,
        )
        .expect("builds reflow patches");
        let applied = apply(stream, &patches).expect("applies surgically");
        let after_str = String::from_utf8_lossy(&applied.bytes);
        assert!(
            after_str
                .contains("BT /F1 10 Tf 1 0 0 1 300 650 Tm (Second column stays identical) Tj ET"),
            "second column bytes are untouched: {after_str}"
        );
    }

    fn shared_header_pdf() -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Contents 5 0 R /Resources 7 0 R>>\nendobj\n",
        );
        offsets.push(out.len());
        out.push_str(
            "4 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Contents 6 0 R /Resources 7 0 R>>\nendobj\n",
        );
        let c1 = "q /Fm0 Do Q";
        offsets.push(out.len());
        let _ = writeln!(
            out,
            "5 0 obj\n<</Length {}>>\nstream\n{c1}\nendstream\nendobj",
            c1.len()
        );
        let c2 = "q /Fm0 Do Q";
        offsets.push(out.len());
        let _ = writeln!(
            out,
            "6 0 obj\n<</Length {}>>\nstream\n{c2}\nendstream\nendobj",
            c2.len()
        );
        offsets.push(out.len());
        out.push_str("7 0 obj\n<</XObject <</Fm0 8 0 R>> /Font <</F1 9 0 R>>>>\nendobj\n");
        let fm = "BT /F1 12 Tf 10 180 Td (Shared Header) Tj ET";
        offsets.push(out.len());
        let _ = writeln!(
            out,
            "8 0 obj\n<</Type /XObject /Subtype /Form /BBox [0 0 200 200] \
             /Resources <</Font <</F1 9 0 R>>>> /Length {}>>\nstream\n{fm}\nendstream\nendobj",
            fm.len()
        );
        offsets.push(out.len());
        out.push_str(
            "9 0 obj\n<</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding>>\nendobj\n",
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

    fn text_on_page(doc: &Document, page_idx: usize) -> String {
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(doc, root).expect("tree");
        let page = tree.get(page_idx).expect("page");
        let res = page
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
            .unwrap_or_default();
        let run = interp::run_with(&ContentStream::parse(&page.decoded_contents(doc)), &res);
        let model = PageModel::build(&run.records);
        page_text(&res, doc, &model)
    }

    #[test]
    fn shared_form_xobject_supports_copy_on_write_and_edit_everywhere() {
        let doc = Document::open(shared_header_pdf(), OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        let p0 = tree.get(0).expect("p0");

        let info = inspect_shared_form(&doc, p0, "Fm0").expect("inspects Fm0");
        assert_eq!(info.pages_using, vec![0, 1]);
        assert!(
            info.prompt
                .as_deref()
                .is_some_and(|p| p.contains("used on 2 pages"))
        );

        let patch = Patch {
            range: 23..38, // "(Shared Header)"
            bytes: b"(Page 1 Header)".to_vec(),
            label: "edit header".to_string(),
        };

        // 1. ThisPageOnly -> copy-on-write: page 0 updates, page 1 stays "Shared Header"!
        let cow_save = edit_shared_form(
            &doc,
            p0,
            "Fm0",
            std::slice::from_ref(&patch),
            SharedFormScope::ThisPageOnly,
        )
        .expect("cow save");
        let doc_cow = Document::open(cow_save.bytes, OpenOptions::default()).expect("reopens cow");
        assert_eq!(text_on_page(&doc_cow, 0), "Page 1 Header");
        assert_eq!(text_on_page(&doc_cow, 1), "Shared Header");

        // 2. Everywhere -> both page 0 and page 1 update!
        let doc_ev_in = Document::open(shared_header_pdf(), OpenOptions::default()).expect("opens");
        let tree_ev = PageTree::build(&doc_ev_in, root).expect("tree");
        let p0_ev = tree_ev.get(0).expect("p0");
        let ev_save = edit_shared_form(
            &doc_ev_in,
            p0_ev,
            "Fm0",
            &[patch],
            SharedFormScope::Everywhere,
        )
        .expect("everywhere save");
        let doc_ev = Document::open(ev_save.bytes, OpenOptions::default()).expect("reopens ev");
        assert_eq!(text_on_page(&doc_ev, 0), "Page 1 Header");
        assert_eq!(text_on_page(&doc_ev, 1), "Page 1 Header");
    }

    #[test]
    fn reflow_refuses_non_winansi_instead_of_emitting_mojibake() {
        let stream = b"BT /F1 10 Tf 1 0 0 1 40 700 Tm (Hello) Tj ET";
        let mut font = mangle_syntax::object::Dict::new();
        font.set("Type", Object::name("Font"));
        font.set("Subtype", Object::name("Type1"));
        font.set("BaseFont", Object::name("Helvetica"));
        font.set("Encoding", Object::name("WinAnsiEncoding"));
        let mut fonts = mangle_syntax::object::Dict::new();
        fonts.set("F1", Object::Dict(font));
        let mut res_dict = mangle_syntax::object::Dict::new();
        res_dict.set("Font", Object::Dict(fonts));
        let resources = Resources::from_dict(&res_dict, &|_| None);

        let before = interp::run_with(&ContentStream::parse(stream), &resources);
        let model = PageModel::build(&before.records);
        let object = &model.objects()[0];

        // WinAnsi Latin-1 (Café) reflows fine.
        let (patches, _) = reflow_patches(
            object,
            "Caf\u{e9} au lait",
            "Helvetica",
            TextAlignment::Left,
        )
        .expect("WinAnsi reflows");
        let applied = apply(stream, &patches).expect("applies");
        assert!(crate::surgery::is_balanced(&applied.bytes));

        // CJK needs a subset embedding via the rung-3 retype path, not raw bytes.
        let err = reflow_patches(
            object,
            "Hello \u{65e5}\u{672c}\u{8a9e}",
            "Helvetica",
            TextAlignment::Left,
        )
        .expect_err("CJK is refused");
        assert!(err.contains("subset embedding"), "honest error: {err}");
    }

    #[test]
    fn shared_form_save_reports_rewritten_and_re_encoded_honestly() {
        let doc = Document::open(shared_header_pdf(), OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        let p0 = tree.get(0).expect("p0");

        let patch = Patch {
            range: 0..0,
            bytes: Vec::new(),
            label: "noop".to_string(),
        };
        // Unfiltered Form: no re-encode needed.
        let cow = edit_shared_form(
            &doc,
            p0,
            "Fm0",
            std::slice::from_ref(&patch),
            SharedFormScope::ThisPageOnly,
        )
        .expect("cow save");
        assert!(
            cow.rewritten.is_empty(),
            "copy-on-write rewrites nothing in place"
        );
        assert_eq!(cow.installed.len(), 3, "copy, resources, and page");
        assert!(!cow.re_encoded, "unfiltered Form needs no re-encode");

        let ev = edit_shared_form(
            &doc,
            p0,
            "Fm0",
            std::slice::from_ref(&patch),
            SharedFormScope::Everywhere,
        )
        .expect("everywhere save");
        assert_eq!(ev.rewritten, vec![8], "shared Form rewritten in place");
    }
}
