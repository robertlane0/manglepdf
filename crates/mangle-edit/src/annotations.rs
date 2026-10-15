//! Annotations, appearance stream (`/AP /N`) generation, FDF/XFDF, and page artifacts
//! (ISO 32000-1 §12.5, GOAL.md §3.3–3.4, §5.1–5.3).

use std::fmt::Write as _;

use mangle_doc::Page;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Name, Object, Rect as PageRect, Ref, Stream};
use mangle_syntax::writer::IncrementalUpdate;

use crate::embed::{EMBEDDED_OBJECTS, embed_truetype};
use crate::writeback::{Save, SaveError, save_decoded_with};

/// Default `/F` flag: Print (bit 3 = 4).
pub const ANNOT_FLAG_PRINT: i64 = 4;

/// Normalize a 4-point quad `[x1, y1, x2, y2, x3, y3, x4, y4]` into Acrobat's
/// `(TL, TR, BL, BR)` order regardless of whether the input was in ISO order or Z-order.
#[must_use]
pub fn normalize_quad(q: [f64; 8]) -> [f64; 8] {
    let mut pts = [(q[0], q[1]), (q[2], q[3]), (q[4], q[5]), (q[6], q[7])];
    // Sort by y descending (top two first, bottom two second).
    pts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (tl, tr) = if pts[0].0 <= pts[1].0 {
        (pts[0], pts[1])
    } else {
        (pts[1], pts[0])
    };
    let (bl, br) = if pts[2].0 <= pts[3].0 {
        (pts[2], pts[3])
    } else {
        (pts[3], pts[2])
    };
    [tl.0, tl.1, tr.0, tr.1, bl.0, bl.1, br.0, br.1]
}

/// Text markup subtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkupKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl MarkupKind {
    #[must_use]
    pub fn subtype_name(self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "StrikeOut",
            Self::Squiggly => "Squiggly",
        }
    }
}

/// Line ending style for `/Line` and `/PolyLine` annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    None,
    OpenArrow,
    ClosedArrow,
    Square,
    Circle,
}

impl LineEnding {
    #[must_use]
    pub fn pdf_name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::OpenArrow => "OpenArrow",
            Self::ClosedArrow => "ClosedArrow",
            Self::Square => "Square",
            Self::Circle => "Circle",
        }
    }
}

/// Link action for a `/Link` annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkAction {
    /// Open an external URI.
    Uri(String),
    /// Jump to a 0-based page index and `(x, y, zoom)` XYZ destination.
    GoTo {
        page_ref: Ref,
        x: f64,
        y: f64,
        zoom: f64,
    },
}

/// Specific geometry and payload of an annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationBody {
    /// Highlight, Underline, StrikeOut, or Squiggly over one or more quads.
    TextMarkup {
        kind: MarkupKind,
        quads: Vec<[f64; 8]>,
    },
    /// Sticky note (`/Subtype /Text`) with an icon name (e.g. `"Comment"`, `"Note"`).
    StickyNote {
        icon: String,
        state: Option<String>,
        in_reply_to: Option<Ref>,
        with_popup: bool,
    },
    /// FreeText box, typewriter, or callout.
    FreeText {
        font_size: f64,
        align: i64,
        callout_line: Option<[f64; 6]>,
    },
    /// Line or arrow between two points in page coordinates.
    Line {
        start: (f64, f64),
        end: (f64, f64),
        start_ending: LineEnding,
        end_ending: LineEnding,
        stroke_width: f64,
    },
    /// Rectangle (`/Subtype /Square`).
    Square {
        stroke_width: f64,
        fill: Option<[f64; 3]>,
    },
    /// Ellipse (`/Subtype /Circle`).
    Circle {
        stroke_width: f64,
        fill: Option<[f64; 3]>,
    },
    /// Closed polygon or cloud (`/Subtype /Polygon`).
    Polygon {
        vertices: Vec<(f64, f64)>,
        stroke_width: f64,
        cloud_intensity: Option<i64>,
    },
    /// Open polyline (`/Subtype /PolyLine`).
    PolyLine {
        vertices: Vec<(f64, f64)>,
        stroke_width: f64,
    },
    /// Multi-stroke freehand ink (`/Subtype /Ink`).
    Ink {
        strokes: Vec<Vec<(f64, f64)>>,
        stroke_width: f64,
    },
    /// Standard or custom text stamp (`/Subtype /Stamp`).
    Stamp { stamp_name: String, label: String },
    /// Caret (`/Subtype /Caret`).
    Caret { in_reply_to: Option<Ref> },
    /// Interactive link (`/Subtype /Link`).
    Link { action: LinkAction },
    /// Redaction mark (`/Subtype /Redact`, unapplied).
    Redact { overlay_text: Option<String> },
}

/// Complete specification of an annotation to create or update.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationSpec {
    /// Bounding box `[left, bottom, right, top]` in page space.
    pub rect: PageRect,
    /// Unique name `/NM`.
    pub name: String,
    /// Author `/T`.
    pub author: String,
    /// Subject `/Subj`.
    pub subject: String,
    /// Text contents `/Contents`.
    pub contents: String,
    /// RGB colour in `0.0..=1.0`.
    pub color: [f64; 3],
    /// Opacity `/CA` in `0.0..=1.0`.
    pub opacity: f64,
    /// `/F` flags (defaults to [`ANNOT_FLAG_PRINT`]).
    pub flags: i64,
    /// Type-specific payload.
    pub body: AnnotationBody,
}

impl AnnotationSpec {
    /// Construct a new annotation spec with sensible defaults.
    #[must_use]
    pub fn new(rect: PageRect, body: AnnotationBody) -> Self {
        Self {
            rect,
            name: "annot-1".to_string(),
            author: "Author".to_string(),
            subject: String::new(),
            contents: String::new(),
            color: [1.0, 0.85, 0.0],
            opacity: 1.0,
            flags: ANNOT_FLAG_PRINT,
            body,
        }
    }
}

/// Result of adding an annotation to a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedAnnotation {
    /// Incremental save containing the new annotation object(s) and updated `/Annots`.
    pub save: Save,
    /// Object reference of the newly created annotation.
    pub annot_ref: Ref,
    /// Object reference of the appearance stream (`/AP /N`), if generated.
    pub ap_ref: Option<Ref>,
}

/// Add a new annotation (with its deterministic `/AP /N` stream) to `page` incrementally,
/// leaving all existing annotations and page streams untouched.
///
/// `system_dirs` configures `--font-dirs` for the Unicode FreeType appearance embedding.
pub fn add_annotation(
    doc: &Document,
    page: &Page,
    spec: &AnnotationSpec,
    system_dirs: &[std::path::PathBuf],
) -> Result<AddedAnnotation, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };

    let mut extra: Vec<(u32, Object)> = Vec::new();
    let annot_num = doc.alloc().num;
    let annot_ref = Ref::new(annot_num, 0);

    let ap_ref = if matches!(spec.body, AnnotationBody::Link { .. }) {
        None
    } else {
        let (ap_stream, ap_extra) = build_appearance_stream(doc, spec, system_dirs);
        extra.extend(ap_extra);
        let ap_num = doc.alloc().num;
        extra.push((ap_num, Object::Stream(ap_stream)));
        Some(Ref::new(ap_num, 0))
    };

    let mut dict = build_annot_dict(spec, ap_ref);
    let mut popup_ref_opt: Option<Ref> = None;

    if let AnnotationBody::StickyNote {
        with_popup: true, ..
    } = &spec.body
    {
        let popup_num = doc.alloc().num;
        let popup_ref = Ref::new(popup_num, 0);
        let mut popup = Dict::new();
        popup.set("Type", Object::name("Annot"));
        popup.set("Subtype", Object::name("Popup"));
        popup.set("Parent", Object::Ref(annot_ref));
        popup.set(
            "Rect",
            rect_array(PageRect::new(
                spec.rect.right + 10.0,
                spec.rect.bottom,
                spec.rect.right + 160.0,
                spec.rect.top + 80.0,
            )),
        );
        popup.set("Open", Object::Bool(true));
        extra.push((popup_num, Object::Dict(popup)));
        dict.set("Popup", Object::Ref(popup_ref));
        popup_ref_opt = Some(popup_ref);
    }

    extra.push((annot_num, Object::Dict(dict)));

    let mut annots = page
        .dict
        .get("Annots")
        .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    annots.push(Object::Ref(annot_ref));
    if let Some(p_ref) = popup_ref_opt {
        annots.push(Object::Ref(p_ref));
    }

    let mut page_dict = page.dict.clone();
    page_dict.set("Annots", Object::Array(annots));
    extra.push((page_ref.num, Object::Dict(page_dict)));

    let save = write_extra_only(doc, &extra);
    Ok(AddedAnnotation {
        save,
        annot_ref,
        ap_ref,
    })
}

/// Delete the annotation at `index` in `/Annots` along with its `/Popup` and `/IRT` replies.
pub fn delete_annotation(doc: &Document, page: &Page, index: usize) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let annots = page
        .dict
        .get("Annots")
        .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    let Some(target) = annots.get(index) else {
        return Ok(Save {
            bytes: doc.bytes().to_vec(),
            rewritten: Vec::new(),
            installed: Vec::new(),
            re_encoded: false,
        });
    };
    let target_num = target.as_ref_id().map(|r| r.num);

    let mut kept = Vec::new();
    for (i, entry) in annots.into_iter().enumerate() {
        if i == index {
            continue;
        }
        if let Some(num) = target_num
            && let Some(resolved) = doc.resolve_object(&entry).or_else(|| Some(entry.clone()))
            && let Some(d) = resolved.as_dict()
        {
            let is_popup_of = d
                .get("Parent")
                .and_then(Object::as_ref_id)
                .is_some_and(|r| r.num == num);
            let is_reply_to = d
                .get("IRT")
                .and_then(Object::as_ref_id)
                .is_some_and(|r| r.num == num);
            if is_popup_of || is_reply_to {
                continue;
            }
        }
        kept.push(entry);
    }

    let mut page_dict = page.dict.clone();
    if kept.is_empty() {
        page_dict.remove("Annots");
    } else {
        page_dict.set("Annots", Object::Array(kept));
    }
    Ok(write_extra_only(
        doc,
        &[(page_ref.num, Object::Dict(page_dict))],
    ))
}

/// Move the annotation at `from` to `to` within the page's `/Annots` array.
pub fn arrange_annotation(
    doc: &Document,
    page: &Page,
    from: usize,
    to: usize,
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let mut annots = page
        .dict
        .get("Annots")
        .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    if from >= annots.len() || to >= annots.len() || from == to {
        return Ok(Save {
            bytes: doc.bytes().to_vec(),
            rewritten: Vec::new(),
            installed: Vec::new(),
            re_encoded: false,
        });
    }
    let item = annots.remove(from);
    annots.insert(to, item);
    let mut page_dict = page.dict.clone();
    page_dict.set("Annots", Object::Array(annots));
    Ok(write_extra_only(
        doc,
        &[(page_ref.num, Object::Dict(page_dict))],
    ))
}

/// Add a `/Artifact` Header or Footer text line to `page` at `(x, y)` in `size` pt.
pub fn add_header_footer(
    doc: &Document,
    page: &Page,
    text: &str,
    x: f64,
    y: f64,
    size: f64,
) -> Result<Save, SaveError> {
    let mut font_dict = Dict::new();
    font_dict.set("Type", Object::name("Font"));
    font_dict.set("Subtype", Object::name("Type1"));
    font_dict.set("BaseFont", Object::name("Helvetica"));
    font_dict.set("Encoding", Object::name("WinAnsiEncoding"));

    let mut literal = Vec::new();
    mangle_syntax::writer::write_literal_string(&mut literal, text.as_bytes());
    let literal_str = String::from_utf8_lossy(&literal);

    let ops = format!(
        "\n/Artifact BMC q BT /F_HF {} Tf {} {} Td 0 0 0 rg {literal_str} Tj ET Q EMC",
        fmt_num(size),
        fmt_num(x),
        fmt_num(y),
    );
    append_page_artifact(
        doc,
        page,
        ops.as_bytes(),
        Some(("F_HF", Object::Dict(font_dict))),
        None,
    )
}

/// Add a removable diagonal `/Artifact` watermark in an `/OCG` `"Watermark"` layer.
pub fn add_watermark(
    doc: &Document,
    page: &Page,
    text: &str,
    opacity: f64,
) -> Result<Save, SaveError> {
    let (w, h) = page.inherited.displayed_size();
    let cx = w * 0.25;
    let cy = h * 0.4;
    let rad = 45.0_f64.to_radians();
    let cos = rad.cos();
    let sin = rad.sin();

    let mut font_dict = Dict::new();
    font_dict.set("Type", Object::name("Font"));
    font_dict.set("Subtype", Object::name("Type1"));
    font_dict.set("BaseFont", Object::name("Helvetica-Bold"));
    font_dict.set("Encoding", Object::name("WinAnsiEncoding"));

    let mut gs_dict = Dict::new();
    gs_dict.set("Type", Object::name("ExtGState"));
    gs_dict.set("ca", Object::Real(opacity.clamp(0.0, 1.0)));
    gs_dict.set("CA", Object::Real(opacity.clamp(0.0, 1.0)));

    let mut literal = Vec::new();
    mangle_syntax::writer::write_literal_string(&mut literal, text.as_bytes());
    let literal_str = String::from_utf8_lossy(&literal);

    let ops = format!(
        "\n/Artifact <</Subtype /Watermark>> BDC q /GS_WM gs 0.5 0.5 0.5 rg \
         BT /F_WM 48 Tf {} {} {} {} {} {} Tm {literal_str} Tj ET Q EMC",
        fmt_num(cos),
        fmt_num(sin),
        fmt_num(-sin),
        fmt_num(cos),
        fmt_num(cx),
        fmt_num(cy),
    );
    append_page_artifact(
        doc,
        page,
        ops.as_bytes(),
        Some(("F_WM", Object::Dict(font_dict))),
        Some(("GS_WM", Object::Dict(gs_dict))),
    )
}

/// Export the page's annotations to XFDF XML.
#[must_use]
pub fn export_xfdf(doc: &Document, page: &Page, page_index: usize) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n\
         <annots>\n",
    );
    for entry in page.annots(doc) {
        let Some(resolved) = doc.resolve_object(&entry).or_else(|| Some(entry.clone())) else {
            continue;
        };
        let Some(d) = resolved.as_dict() else {
            continue;
        };
        let subtype = d
            .get("Subtype")
            .and_then(Object::as_name)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        if subtype == "Popup" {
            continue;
        }
        let rect = d
            .get("Rect")
            .and_then(|r| {
                PageRect::from_object(&doc.resolve_object(r).unwrap_or_else(|| r.clone())).ok()
            })
            .unwrap_or_else(|| PageRect::new(0.0, 0.0, 0.0, 0.0));
        let name = d
            .get("NM")
            .and_then(Object::as_bytes)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let author = d
            .get("T")
            .and_then(Object::as_bytes)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let subject = d
            .get("Subj")
            .and_then(Object::as_bytes)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let contents = d
            .get("Contents")
            .and_then(Object::as_bytes)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  <annot subtype=\"{subtype}\" page=\"{page_index}\" \
             rect=\"{},{},{},{}\" name=\"{name}\" title=\"{author}\" \
             subject=\"{subject}\" contents=\"{contents}\"/>",
            fmt_num(rect.left),
            fmt_num(rect.bottom),
            fmt_num(rect.right),
            fmt_num(rect.top),
        );
    }
    out.push_str("</annots>\n</xfdf>\n");
    out
}

/// Import annotations from an XFDF string produced by [`export_xfdf`] onto `page`.
pub fn import_xfdf(
    doc: &Document,
    page: &Page,
    xfdf: &str,
    system_dirs: &[std::path::PathBuf],
) -> Result<Save, SaveError> {
    let mut current_bytes = doc.bytes().to_vec();
    for line in xfdf.lines() {
        let line = line.trim();
        if !line.starts_with("<annot ") {
            continue;
        }
        let subtype = xml_attr(line, "subtype").unwrap_or_default();
        let rect_str = xml_attr(line, "rect").unwrap_or_default();
        let nums: Vec<f64> = rect_str
            .split(',')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .collect();
        let rect = if let [l, b, r, t] = nums.as_slice() {
            PageRect::new(*l, *b, *r, *t)
        } else {
            continue;
        };
        let body = match subtype.as_str() {
            "Highlight" => AnnotationBody::TextMarkup {
                kind: MarkupKind::Highlight,
                quads: vec![[
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.top,
                    rect.left,
                    rect.bottom,
                    rect.right,
                    rect.bottom,
                ]],
            },
            "Underline" => AnnotationBody::TextMarkup {
                kind: MarkupKind::Underline,
                quads: vec![[
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.top,
                    rect.left,
                    rect.bottom,
                    rect.right,
                    rect.bottom,
                ]],
            },
            "StrikeOut" => AnnotationBody::TextMarkup {
                kind: MarkupKind::StrikeOut,
                quads: vec![[
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.top,
                    rect.left,
                    rect.bottom,
                    rect.right,
                    rect.bottom,
                ]],
            },
            "Squiggly" => AnnotationBody::TextMarkup {
                kind: MarkupKind::Squiggly,
                quads: vec![[
                    rect.left,
                    rect.top,
                    rect.right,
                    rect.top,
                    rect.left,
                    rect.bottom,
                    rect.right,
                    rect.bottom,
                ]],
            },
            "Text" => AnnotationBody::StickyNote {
                icon: "Comment".to_string(),
                state: None,
                in_reply_to: None,
                with_popup: false,
            },
            "FreeText" => AnnotationBody::FreeText {
                font_size: 12.0,
                align: 0,
                callout_line: None,
            },
            "Circle" => AnnotationBody::Circle {
                stroke_width: 1.0,
                fill: None,
            },
            _ => AnnotationBody::Square {
                stroke_width: 1.0,
                fill: None,
            },
        };
        let mut spec = AnnotationSpec::new(rect, body);
        spec.name = xml_attr(line, "name").unwrap_or_else(|| "annot".to_string());
        spec.author = xml_attr(line, "title").unwrap_or_default();
        spec.subject = xml_attr(line, "subject").unwrap_or_default();
        spec.contents = xml_attr(line, "contents").unwrap_or_default();

        let cur_doc = Document::open(current_bytes.clone(), mangle_syntax::OpenOptions::default())
            .map_err(|_| SaveError::ContentsNotIndirect)?;
        let cur_page = if let Some(r) = page.object_ref()
            && let Some(Object::Dict(d)) = cur_doc.object(r)
        {
            let mut p = page.clone();
            p.dict = d;
            p
        } else {
            page.clone()
        };
        let added = add_annotation(&cur_doc, &cur_page, &spec, system_dirs)?;
        current_bytes = added.save.bytes;
    }
    Ok(Save {
        bytes: current_bytes,
        rewritten: Vec::new(),
        installed: Vec::new(),
        re_encoded: false,
    })
}

fn xml_attr(line: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=\"");
    let start = line.find(&pat)? + pat.len();
    let rest = line.get(start..)?;
    let end = rest.find('"')?;
    Some(rest.get(..end)?.to_string())
}

fn build_annot_dict(spec: &AnnotationSpec, ap_ref: Option<Ref>) -> Dict {
    let mut d = Dict::new();
    d.set("Type", Object::name("Annot"));
    let subtype = match &spec.body {
        AnnotationBody::TextMarkup { kind, .. } => kind.subtype_name(),
        AnnotationBody::StickyNote { .. } => "Text",
        AnnotationBody::FreeText { .. } => "FreeText",
        AnnotationBody::Line { .. } => "Line",
        AnnotationBody::Square { .. } => "Square",
        AnnotationBody::Circle { .. } => "Circle",
        AnnotationBody::Polygon { .. } => "Polygon",
        AnnotationBody::PolyLine { .. } => "PolyLine",
        AnnotationBody::Ink { .. } => "Ink",
        AnnotationBody::Stamp { .. } => "Stamp",
        AnnotationBody::Caret { .. } => "Caret",
        AnnotationBody::Link { .. } => "Link",
        AnnotationBody::Redact { .. } => "Redact",
    };
    d.set("Subtype", Object::name(subtype));
    d.set("Rect", rect_array(spec.rect));
    d.set("NM", Object::string(&spec.name));
    if !spec.author.is_empty() {
        d.set("T", Object::string(&spec.author));
    }
    if !spec.subject.is_empty() {
        d.set("Subj", Object::string(&spec.subject));
    }
    if !spec.contents.is_empty() {
        d.set("Contents", Object::string(&spec.contents));
    }
    d.set("F", Object::Int(spec.flags));
    d.set(
        "C",
        Object::Array(vec![
            Object::Real(spec.color[0]),
            Object::Real(spec.color[1]),
            Object::Real(spec.color[2]),
        ]),
    );
    d.set("CA", Object::Real(spec.opacity));
    d.set("M", Object::string("D:20260101000000Z"));
    d.set("CreationDate", Object::string("D:20260101000000Z"));

    if let Some(ap) = ap_ref {
        let mut ap_dict = Dict::new();
        ap_dict.set("N", Object::Ref(ap));
        d.set("AP", Object::Dict(ap_dict));
    }

    match &spec.body {
        AnnotationBody::TextMarkup { quads, .. } => {
            let mut arr = Vec::with_capacity(quads.len() * 8);
            for q in quads {
                for v in normalize_quad(*q) {
                    arr.push(Object::Real(v));
                }
            }
            d.set("QuadPoints", Object::Array(arr));
        }
        AnnotationBody::StickyNote {
            icon,
            state,
            in_reply_to,
            ..
        } => {
            d.set("Name", Object::name(icon));
            if let Some(st) = state {
                d.set("State", Object::string(st));
                d.set("StateModel", Object::string("Review"));
            }
            if let Some(irt) = in_reply_to {
                d.set("IRT", Object::Ref(*irt));
                d.set("RT", Object::name("R"));
            }
        }
        AnnotationBody::FreeText {
            font_size,
            align,
            callout_line,
        } => {
            let da = format!(
                "/F1 {} Tf {} {} {} rg",
                fmt_num(*font_size),
                fmt_num(spec.color[0]),
                fmt_num(spec.color[1]),
                fmt_num(spec.color[2])
            );
            d.set("DA", Object::string(&da));
            d.set("Q", Object::Int(*align));
            if let Some(cl) = callout_line {
                d.set(
                    "CL",
                    Object::Array(cl.iter().map(|v| Object::Real(*v)).collect()),
                );
                d.set("IT", Object::name("FreeTextCallout"));
            }
        }
        AnnotationBody::Line {
            start,
            end,
            start_ending,
            end_ending,
            ..
        } => {
            d.set(
                "L",
                Object::Array(vec![
                    Object::Real(start.0),
                    Object::Real(start.1),
                    Object::Real(end.0),
                    Object::Real(end.1),
                ]),
            );
            d.set(
                "LE",
                Object::Array(vec![
                    Object::name(start_ending.pdf_name()),
                    Object::name(end_ending.pdf_name()),
                ]),
            );
        }
        AnnotationBody::Polygon {
            vertices,
            cloud_intensity,
            ..
        } => {
            let mut v = Vec::with_capacity(vertices.len() * 2);
            for (x, y) in vertices {
                v.push(Object::Real(*x));
                v.push(Object::Real(*y));
            }
            d.set("Vertices", Object::Array(v));
            if let Some(intensity) = cloud_intensity {
                let mut be = Dict::new();
                be.set("S", Object::name("C"));
                be.set("I", Object::Int(*intensity));
                d.set("BE", Object::Dict(be));
            }
        }
        AnnotationBody::PolyLine { vertices, .. } => {
            let mut v = Vec::with_capacity(vertices.len() * 2);
            for (x, y) in vertices {
                v.push(Object::Real(*x));
                v.push(Object::Real(*y));
            }
            d.set("Vertices", Object::Array(v));
        }
        AnnotationBody::Ink { strokes, .. } => {
            let list: Vec<Object> = strokes
                .iter()
                .map(|stroke| {
                    let mut pts = Vec::with_capacity(stroke.len() * 2);
                    for (x, y) in stroke {
                        pts.push(Object::Real(*x));
                        pts.push(Object::Real(*y));
                    }
                    Object::Array(pts)
                })
                .collect();
            d.set("InkList", Object::Array(list));
        }
        AnnotationBody::Stamp { stamp_name, .. } => {
            d.set("Name", Object::name(stamp_name));
        }
        AnnotationBody::Caret { in_reply_to } => {
            if let Some(irt) = in_reply_to {
                d.set("IRT", Object::Ref(*irt));
                d.set("RT", Object::name("R"));
            }
        }
        AnnotationBody::Link { action } => {
            let mut act = Dict::new();
            act.set("Type", Object::name("Action"));
            match action {
                LinkAction::Uri(uri) => {
                    act.set("S", Object::name("URI"));
                    act.set("URI", Object::string(uri));
                }
                LinkAction::GoTo {
                    page_ref,
                    x,
                    y,
                    zoom,
                } => {
                    act.set("S", Object::name("GoTo"));
                    act.set(
                        "D",
                        Object::Array(vec![
                            Object::Ref(*page_ref),
                            Object::name("XYZ"),
                            Object::Real(*x),
                            Object::Real(*y),
                            Object::Real(*zoom),
                        ]),
                    );
                }
            }
            d.set("A", Object::Dict(act));
        }
        AnnotationBody::Redact { overlay_text } => {
            d.set(
                "IC",
                Object::Array(vec![
                    Object::Real(spec.color[0]),
                    Object::Real(spec.color[1]),
                    Object::Real(spec.color[2]),
                ]),
            );
            if let Some(txt) = overlay_text {
                d.set("OverlayText", Object::string(txt));
            }
        }
        AnnotationBody::Square { fill, .. } | AnnotationBody::Circle { fill, .. } => {
            if let Some(ic) = fill {
                d.set(
                    "IC",
                    Object::Array(vec![
                        Object::Real(ic[0]),
                        Object::Real(ic[1]),
                        Object::Real(ic[2]),
                    ]),
                );
            }
        }
    }
    d
}

/// Build the deterministic `/AP /N` Form XObject stream (plus any embedded font objects).
fn build_appearance_stream(
    doc: &Document,
    spec: &AnnotationSpec,
    system_dirs: &[std::path::PathBuf],
) -> (Stream, Vec<(u32, Object)>) {
    let mut dict = Dict::new();
    dict.set("Type", Object::name("XObject"));
    dict.set("Subtype", Object::name("Form"));
    dict.set("BBox", rect_array(spec.rect));

    let mut res = Dict::new();
    let mut gs_table = Dict::new();
    let mut gs0 = Dict::new();
    gs0.set("Type", Object::name("ExtGState"));
    let bm = if matches!(
        spec.body,
        AnnotationBody::TextMarkup {
            kind: MarkupKind::Highlight,
            ..
        }
    ) {
        "Multiply"
    } else {
        "Normal"
    };
    gs0.set("BM", Object::name(bm));
    gs0.set("ca", Object::Real(spec.opacity));
    gs0.set("CA", Object::Real(spec.opacity));
    gs_table.set("GS0", Object::Dict(gs0));
    res.set("ExtGState", Object::Dict(gs_table));

    let mut extra = Vec::new();
    let (red, green, blue) = (
        fmt_num(spec.color[0]),
        fmt_num(spec.color[1]),
        fmt_num(spec.color[2]),
    );
    let mut ops = format!("q /GS0 gs {red} {green} {blue} rg {red} {green} {blue} RG ");

    match &spec.body {
        AnnotationBody::TextMarkup { kind, quads } => {
            for q in quads {
                let nq = normalize_quad(*q);
                let (tl_x, tl_y, tr_x, tr_y, bl_x, bl_y, br_x, br_y) =
                    (nq[0], nq[1], nq[2], nq[3], nq[4], nq[5], nq[6], nq[7]);
                match kind {
                    MarkupKind::Highlight => {
                        let _ = write!(
                            ops,
                            "{} {} m {} {} l {} {} l {} {} l h f ",
                            fmt_num(tl_x),
                            fmt_num(tl_y),
                            fmt_num(tr_x),
                            fmt_num(tr_y),
                            fmt_num(br_x),
                            fmt_num(br_y),
                            fmt_num(bl_x),
                            fmt_num(bl_y),
                        );
                    }
                    MarkupKind::Underline => {
                        let _ = write!(
                            ops,
                            "1 w {} {} m {} {} l S ",
                            fmt_num(bl_x),
                            fmt_num(bl_y),
                            fmt_num(br_x),
                            fmt_num(br_y),
                        );
                    }
                    MarkupKind::StrikeOut => {
                        let my_l = f64::midpoint(tl_y, bl_y);
                        let my_r = f64::midpoint(tr_y, br_y);
                        let _ = write!(
                            ops,
                            "1 w {} {} m {} {} l S ",
                            fmt_num(bl_x),
                            fmt_num(my_l),
                            fmt_num(br_x),
                            fmt_num(my_r),
                        );
                    }
                    MarkupKind::Squiggly => {
                        let _ = write!(ops, "1 w {} {} m ", fmt_num(bl_x), fmt_num(bl_y));
                        let mut x = bl_x;
                        let mut up = true;
                        while x < br_x {
                            x = (x + 3.0).min(br_x);
                            let y = if up { bl_y + 1.5 } else { bl_y };
                            up = !up;
                            let _ = write!(ops, "{} {} l ", fmt_num(x), fmt_num(y));
                        }
                        ops.push_str("S ");
                    }
                }
            }
        }
        AnnotationBody::StickyNote { .. } | AnnotationBody::Caret { .. } => {
            let _ = write!(
                ops,
                "{} {} {} {} re f ",
                fmt_num(spec.rect.left),
                fmt_num(spec.rect.bottom),
                fmt_num(spec.rect.width()),
                fmt_num(spec.rect.height()),
            );
        }
        AnnotationBody::FreeText { font_size, .. } => {
            let mut font_table = Dict::new();
            let chars: Vec<char> = spec.contents.chars().collect();
            let needs_unicode = chars.iter().any(|c| !c.is_ascii());
            let fallback = needs_unicode
                .then(|| mangle_font::resolve_fallback("Helvetica", system_dirs))
                .flatten();
            if let Some(face) = fallback.as_ref() {
                let first = doc.alloc().num;
                for _ in 1..EMBEDDED_OBJECTS {
                    doc.alloc();
                }
                if let Ok(embedded) = embed_truetype(&face.bytes, &chars, &face.family, first) {
                    font_table.set("F1", Object::Ref(Ref::new(embedded.type0, 0)));
                    for (num, obj) in embedded.numbered {
                        extra.push((num, obj));
                    }
                    let line_h = (*font_size * 1.2).max(1.0);
                    let mut y = spec.rect.top - *font_size;
                    for line in spec.contents.lines() {
                        let mut raw = Vec::new();
                        for ch in line.chars() {
                            if let Some((_, cid)) = embedded.cids.iter().find(|(c, _)| *c == ch) {
                                raw.push((*cid >> 8) as u8);
                                raw.push((*cid & 0xFF) as u8);
                            }
                        }
                        let mut lit = Vec::new();
                        mangle_syntax::writer::write_literal_string(&mut lit, &raw);
                        let lit_s = String::from_utf8_lossy(&lit);
                        let _ = write!(
                            ops,
                            "BT /F1 {} Tf {} {} Td {lit_s} Tj ET ",
                            fmt_num(*font_size),
                            fmt_num(spec.rect.left + 2.0),
                            fmt_num(y),
                        );
                        y -= line_h;
                    }
                }
            } else {
                let mut f1 = Dict::new();
                f1.set("Type", Object::name("Font"));
                f1.set("Subtype", Object::name("Type1"));
                f1.set("BaseFont", Object::name("Helvetica"));
                f1.set("Encoding", Object::name("WinAnsiEncoding"));
                font_table.set("F1", Object::Dict(f1));
                let line_h = (*font_size * 1.2).max(1.0);
                let mut y = spec.rect.top - *font_size;
                for line in spec.contents.lines() {
                    let mut lit = Vec::new();
                    mangle_syntax::writer::write_literal_string(&mut lit, line.as_bytes());
                    let lit_s = String::from_utf8_lossy(&lit);
                    let _ = write!(
                        ops,
                        "BT /F1 {} Tf {} {} Td {lit_s} Tj ET ",
                        fmt_num(*font_size),
                        fmt_num(spec.rect.left + 2.0),
                        fmt_num(y),
                    );
                    y -= line_h;
                }
            }
            res.set("Font", Object::Dict(font_table));
        }
        AnnotationBody::Line {
            start,
            end,
            stroke_width,
            ..
        } => {
            let _ = write!(
                ops,
                "{} w {} {} m {} {} l S ",
                fmt_num(*stroke_width),
                fmt_num(start.0),
                fmt_num(start.1),
                fmt_num(end.0),
                fmt_num(end.1),
            );
        }
        AnnotationBody::Square { stroke_width, fill } => {
            if let Some(ic) = fill {
                let _ = write!(
                    ops,
                    "{} {} {} rg {} {} {} {} re B ",
                    fmt_num(ic[0]),
                    fmt_num(ic[1]),
                    fmt_num(ic[2]),
                    fmt_num(spec.rect.left),
                    fmt_num(spec.rect.bottom),
                    fmt_num(spec.rect.width()),
                    fmt_num(spec.rect.height()),
                );
            } else {
                let _ = write!(
                    ops,
                    "{} w {} {} {} {} re S ",
                    fmt_num(*stroke_width),
                    fmt_num(spec.rect.left),
                    fmt_num(spec.rect.bottom),
                    fmt_num(spec.rect.width()),
                    fmt_num(spec.rect.height()),
                );
            }
        }
        AnnotationBody::Circle { stroke_width, fill } => {
            let cx = f64::midpoint(spec.rect.left, spec.rect.right);
            let cy = f64::midpoint(spec.rect.bottom, spec.rect.top);
            let rx = spec.rect.width() * 0.5;
            let ry = spec.rect.height() * 0.5;
            let kx = 0.552_284_75 * rx;
            let ky = 0.552_284_75 * ry;
            let _ = write!(
                ops,
                "{} w {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c \
                 {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
                fmt_num(*stroke_width),
                fmt_num(cx + rx),
                fmt_num(cy),
                fmt_num(cx + rx),
                fmt_num(cy + ky),
                fmt_num(cx + kx),
                fmt_num(cy + ry),
                fmt_num(cx),
                fmt_num(cy + ry),
                fmt_num(cx - kx),
                fmt_num(cy + ry),
                fmt_num(cx - rx),
                fmt_num(cy + ky),
                fmt_num(cx - rx),
                fmt_num(cy),
                fmt_num(cx - rx),
                fmt_num(cy - ky),
                fmt_num(cx - kx),
                fmt_num(cy - ry),
                fmt_num(cx),
                fmt_num(cy - ry),
                fmt_num(cx + kx),
                fmt_num(cy - ry),
                fmt_num(cx + rx),
                fmt_num(cy - ky),
                fmt_num(cx + rx),
                fmt_num(cy),
            );
            if let Some(ic) = fill {
                let _ = write!(
                    ops,
                    "{} {} {} rg B ",
                    fmt_num(ic[0]),
                    fmt_num(ic[1]),
                    fmt_num(ic[2])
                );
            } else {
                ops.push_str("S ");
            }
        }
        AnnotationBody::Polygon {
            vertices,
            stroke_width,
            ..
        } => {
            if let Some((first, rest)) = vertices.split_first() {
                let _ = write!(
                    ops,
                    "{} w {} {} m ",
                    fmt_num(*stroke_width),
                    fmt_num(first.0),
                    fmt_num(first.1)
                );
                for (x, y) in rest {
                    let _ = write!(ops, "{} {} l ", fmt_num(*x), fmt_num(*y));
                }
                ops.push_str("h S ");
            }
        }
        AnnotationBody::PolyLine {
            vertices,
            stroke_width,
        } => {
            if let Some((first, rest)) = vertices.split_first() {
                let _ = write!(
                    ops,
                    "{} w {} {} m ",
                    fmt_num(*stroke_width),
                    fmt_num(first.0),
                    fmt_num(first.1)
                );
                for (x, y) in rest {
                    let _ = write!(ops, "{} {} l ", fmt_num(*x), fmt_num(*y));
                }
                ops.push_str("S ");
            }
        }
        AnnotationBody::Ink {
            strokes,
            stroke_width,
        } => {
            let _ = write!(ops, "{} w ", fmt_num(*stroke_width));
            for stroke in strokes {
                if let Some((first, rest)) = stroke.split_first() {
                    let _ = write!(ops, "{} {} m ", fmt_num(first.0), fmt_num(first.1));
                    for (x, y) in rest {
                        let _ = write!(ops, "{} {} l ", fmt_num(*x), fmt_num(*y));
                    }
                    ops.push_str("S ");
                }
            }
        }
        AnnotationBody::Stamp { label, .. } => {
            let mut f1 = Dict::new();
            f1.set("Type", Object::name("Font"));
            f1.set("Subtype", Object::name("Type1"));
            f1.set("BaseFont", Object::name("Helvetica-Bold"));
            f1.set("Encoding", Object::name("WinAnsiEncoding"));
            let mut font_table = Dict::new();
            font_table.set("F1", Object::Dict(f1));
            res.set("Font", Object::Dict(font_table));
            let mut lit = Vec::new();
            mangle_syntax::writer::write_literal_string(&mut lit, label.as_bytes());
            let lit_s = String::from_utf8_lossy(&lit);
            let _ = write!(
                ops,
                "2 w {} {} {} {} re S BT /F1 14 Tf {} {} Td {lit_s} Tj ET ",
                fmt_num(spec.rect.left + 1.0),
                fmt_num(spec.rect.bottom + 1.0),
                fmt_num((spec.rect.width() - 2.0).max(1.0)),
                fmt_num((spec.rect.height() - 2.0).max(1.0)),
                fmt_num(spec.rect.left + 6.0),
                fmt_num(spec.rect.bottom + 6.0),
            );
        }
        AnnotationBody::Redact { .. } => {
            let _ = write!(
                ops,
                "1 w {} {} {} {} re S ",
                fmt_num(spec.rect.left),
                fmt_num(spec.rect.bottom),
                fmt_num(spec.rect.width()),
                fmt_num(spec.rect.height()),
            );
        }
        AnnotationBody::Link { .. } => {}
    }

    ops.push('Q');
    dict.set("Resources", Object::Dict(res));
    (Stream::new(dict, ops.into_bytes()), extra)
}

fn append_page_artifact(
    doc: &Document,
    page: &Page,
    snippet: &[u8],
    font_entry: Option<(&str, Object)>,
    gs_entry: Option<(&str, Object)>,
) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let Some(page_ref) = page.object_ref() else {
        return Err(SaveError::ContentsNotIndirect);
    };
    let mut res_dict = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    if let Some((f_name, f_obj)) = font_entry {
        let mut fonts = res_dict
            .get("Font")
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        fonts.insert(Name::new(f_name), f_obj);
        res_dict.set("Font", Object::Dict(fonts));
    }
    if let Some((gs_name, gs_obj)) = gs_entry {
        let mut gs = res_dict
            .get("ExtGState")
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        gs.insert(Name::new(gs_name), gs_obj);
        res_dict.set("ExtGState", Object::Dict(gs));
    }

    let mut extra = Vec::new();
    let mut page_dict = page.dict.clone();
    match &page.inherited.resources {
        Some(Object::Ref(r)) => extra.push((r.num, Object::Dict(res_dict))),
        _ => {
            let res_num = doc.alloc().num;
            extra.push((res_num, Object::Dict(res_dict)));
            page_dict.set("Resources", Object::Ref(Ref::new(res_num, 0)));
            extra.push((page_ref.num, Object::Dict(page_dict)));
        }
    }

    let parts = crate::writeback::parts_of(doc, page)?;
    let mut wanted: Vec<Option<Vec<u8>>> = vec![None; parts.len()];
    if let Some((last_idx, last_part)) = parts.iter().enumerate().next_back() {
        let mut updated = last_part.decoded.clone();
        updated.extend_from_slice(snippet);
        if let Some(slot) = wanted.get_mut(last_idx) {
            *slot = Some(updated);
        }
    }
    save_decoded_with(doc, page, &wanted, &extra)
}

fn write_extra_only(doc: &Document, extra: &[(u32, Object)]) -> Save {
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

fn rect_array(r: PageRect) -> Object {
    Object::Array(vec![
        Object::Real(r.left),
        Object::Real(r.bottom),
        Object::Real(r.right),
        Object::Real(r.top),
    ])
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
    use mangle_render::{RenderOptions, render_page};
    use mangle_syntax::object::Rect as PageRect;
    use mangle_syntax::{Document, OpenOptions};

    use super::{
        AnnotationBody, AnnotationSpec, MarkupKind, add_annotation, add_header_footer,
        add_watermark, arrange_annotation, delete_annotation, export_xfdf, import_xfdf,
        normalize_quad,
    };

    fn blank_pdf() -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Contents 4 0 R /Resources 5 0 R>>\nendobj\n",
        );
        let body = "q Q";
        offsets.push(out.len());
        let _ = write!(
            out,
            "4 0 obj\n<</Length {}>>\nstream\n{body}\nendstream\nendobj\n",
            body.len()
        );
        offsets.push(out.len());
        out.push_str("5 0 obj\n<<>>\nendobj\n");
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

    fn open_page(bytes: Vec<u8>) -> (Document, mangle_doc::Page, Resources) {
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
    fn quad_points_normalize_to_tl_tr_bl_br() {
        let iso = [10.0, 20.0, 90.0, 20.0, 90.0, 40.0, 10.0, 40.0];
        assert_eq!(
            normalize_quad(iso),
            [10.0, 40.0, 90.0, 40.0, 10.0, 20.0, 90.0, 20.0]
        );
    }

    #[test]
    fn highlight_and_unicode_freetext_generate_appearances_and_render() {
        let (doc, page, _) = open_page(blank_pdf());
        let hl = AnnotationSpec::new(
            PageRect::new(20.0, 100.0, 140.0, 120.0),
            AnnotationBody::TextMarkup {
                kind: MarkupKind::Highlight,
                quads: vec![[20.0, 100.0, 140.0, 100.0, 140.0, 120.0, 20.0, 120.0]],
            },
        );
        let added = add_annotation(&doc, &page, &hl, &[]).expect("adds highlight");
        assert!(added.ap_ref.is_some());

        let (doc2, page2, _) = open_page(added.save.bytes);
        let mut ft = AnnotationSpec::new(
            PageRect::new(20.0, 40.0, 180.0, 80.0),
            AnnotationBody::FreeText {
                font_size: 12.0,
                align: 0,
                callout_line: None,
            },
        );
        ft.contents = "Café — Ζωή €".to_string();
        let added2 = add_annotation(&doc2, &page2, &ft, &[]).expect("adds unicode freetext");

        let (doc3, page3, res3) = open_page(added2.save.bytes);
        assert_eq!(page3.annots(&doc3).len(), 2);
        let rendered = render_page(&doc3, &page3, &res3, RenderOptions::default());
        assert!(rendered.notes.is_empty(), "notes: {:?}", rendered.notes);
    }

    /// A configured `--font-dirs` names the embedded appearance font.
    ///
    /// Same Unicode FreeText as above, but resolved through the repository's own
    /// bundled directory standing in as a system directory: the installed font's
    /// `/BaseFont` carries the file's own family (`Liberation Sans`) rather than
    /// the bundled alias, proving the configured file was embedded.
    #[test]
    fn a_configured_font_dir_names_the_embedded_appearance_font() {
        let (doc, page, _) = open_page(blank_pdf());
        let mut ft = AnnotationSpec::new(
            PageRect::new(20.0, 40.0, 180.0, 80.0),
            AnnotationBody::FreeText {
                font_size: 12.0,
                align: 0,
                callout_line: None,
            },
        );
        ft.contents = "Café — Ζωή €".to_string();
        let dirs = [std::path::PathBuf::from("../../assets/fonts")];
        let added = add_annotation(&doc, &page, &ft, &dirs).expect("adds freetext");
        let doc2 = Document::open(added.save.bytes, OpenOptions::default()).expect("reopens");
        let mut names = Vec::new();
        for num in doc2.object_numbers() {
            let Some(obj) = doc2.object(mangle_syntax::object::Ref::new(num, 0)) else {
                continue;
            };
            let Some(dict) = obj.as_dict() else {
                continue;
            };
            if let Some(base) = dict
                .get("BaseFont")
                .and_then(mangle_syntax::object::Object::as_name)
            {
                names.push(String::from_utf8_lossy(base).into_owned());
            }
        }
        assert!(
            names.iter().any(|n| n.contains("Liberation Sans")),
            "the system file's own family was embedded: {names:?}"
        );
    }

    #[test]
    fn sticky_note_delete_removes_popup_and_replies_and_xfdf_round_trips() {
        let (doc, page, _) = open_page(blank_pdf());
        let mut note = AnnotationSpec::new(
            PageRect::new(30.0, 30.0, 54.0, 54.0),
            AnnotationBody::StickyNote {
                icon: "Comment".to_string(),
                state: Some("Accepted".to_string()),
                in_reply_to: None,
                with_popup: true,
            },
        );
        note.author = "Reviewer".to_string();
        note.contents = "Looks good".to_string();
        let added = add_annotation(&doc, &page, &note, &[]).expect("adds note + popup");

        let (doc2, page2, _) = open_page(added.save.bytes);
        let reply = AnnotationSpec::new(
            PageRect::new(30.0, 30.0, 54.0, 54.0),
            AnnotationBody::StickyNote {
                icon: "Note".to_string(),
                state: None,
                in_reply_to: Some(added.annot_ref),
                with_popup: false,
            },
        );
        let added_reply = add_annotation(&doc2, &page2, &reply, &[]).expect("adds reply");

        let (doc3, page3, _) = open_page(added_reply.save.bytes);
        assert_eq!(page3.annots(&doc3).len(), 3, "note, popup, and reply");

        let xfdf = export_xfdf(&doc3, &page3, 0);
        assert!(xfdf.contains("Reviewer"));
        assert!(xfdf.contains("Looks good"));

        let (fresh_doc, fresh_page, _) = open_page(blank_pdf());
        let imported = import_xfdf(&fresh_doc, &fresh_page, &xfdf, &[]).expect("imports xfdf");
        let (doc_imp, page_imp, _) = open_page(imported.bytes);
        assert_eq!(page_imp.annots(&doc_imp).len(), 2);

        // Arrange and delete parent note -> removes note, popup, and reply.
        let arranged = arrange_annotation(&doc3, &page3, 0, 2).expect("arranges");
        let (doc4, page4, _) = open_page(arranged.bytes);
        let deleted = delete_annotation(&doc4, &page4, 2).expect("deletes parent");
        let (doc5, page5, _) = open_page(deleted.bytes);
        assert_eq!(
            page5.annots(&doc5).len(),
            0,
            "popup and reply pruned with parent"
        );
    }

    #[test]
    fn header_footer_and_watermark_artifacts_render_cleanly() {
        let (doc, page, _) = open_page(blank_pdf());
        let hf = add_header_footer(&doc, &page, "TravelGuide - Page 1", 20.0, 185.0, 10.0)
            .expect("adds header");
        let (doc2, page2, _) = open_page(hf.bytes);
        let wm = add_watermark(&doc2, &page2, "DRAFT", 0.3).expect("adds watermark");
        let (doc3, page3, res3) = open_page(wm.bytes);
        let rendered = render_page(&doc3, &page3, &res3, RenderOptions::default());
        assert!(rendered.marks >= 2);
    }
}
