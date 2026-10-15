//! Interactive AcroForms: filling, `AF*` calculation/formatting/validation, Prepare Form,
//! FDF/XFDF data exchange, and XFA detection (ISO 32000-1 §12.7, GOAL.md §3.5, FINISH.md S6).

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use mangle_doc::Page;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Object, Rect as PageRect, Ref, Stream};
use mangle_syntax::writer::IncrementalUpdate;

use crate::embed::{EMBEDDED_OBJECTS, embed_truetype};
use crate::writeback::{Save, SaveError};

// ---- Field flags (ISO 32000-1 Tables 221, 226, 228, 230) ----

pub const FF_READ_ONLY: i64 = 1 << 0;
pub const FF_REQUIRED: i64 = 1 << 1;
pub const FF_MULTILINE: i64 = 1 << 12;
pub const FF_PASSWORD: i64 = 1 << 13;
pub const FF_NO_TOGGLE_TO_OFF: i64 = 1 << 14;
pub const FF_RADIO: i64 = 1 << 15;
pub const FF_PUSHBUTTON: i64 = 1 << 16;
pub const FF_COMBO: i64 = 1 << 17;
pub const FF_EDIT: i64 = 1 << 18;
pub const FF_MULTI_SELECT: i64 = 1 << 21;
pub const FF_COMB: i64 = 1 << 24;
pub const FF_RADIOS_IN_UNISON: i64 = 1 << 25;

/// Standard Acrobat `AFSimple_Calculate` aggregation operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfCalcOp {
    Sum,
    Prd,
    Avg,
    Min,
    Max,
}

impl AfCalcOp {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "SUM" => Some(Self::Sum),
            "PRD" => Some(Self::Prd),
            "AVG" | "AVE" => Some(Self::Avg),
            "MIN" => Some(Self::Min),
            "MAX" => Some(Self::Max),
            _ => None,
        }
    }

    #[must_use]
    pub fn evaluate(self, values: &[f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        match self {
            Self::Sum => values.iter().sum(),
            Self::Prd => values.iter().product(),
            Self::Avg => values.iter().sum::<f64>() / (values.len() as f64),
            Self::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
            Self::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

/// Standard `AFNumber_Format` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfNumberFormat {
    pub decimals: usize,
    pub thousands_sep: bool,
    pub currency: String,
    pub prepend_currency: bool,
}

impl AfNumberFormat {
    /// Format a numeric value according to `AFNumber_Format`.
    #[must_use]
    pub fn format(&self, value: f64) -> String {
        let neg = value < 0.0;
        let abs = value.abs();
        let raw = format!("{abs:.prec$}", prec = self.decimals);
        let (int_part, frac_part) = raw.split_once('.').unwrap_or((&raw, ""));
        let grouped = if self.thousands_sep {
            group_thousands(int_part)
        } else {
            int_part.to_string()
        };
        let number = if self.decimals > 0 {
            format!("{grouped}.{frac_part}")
        } else {
            grouped
        };
        let with_curr = if self.currency.is_empty() {
            number
        } else if self.prepend_currency {
            format!("{}{number}", self.currency)
        } else {
            format!("{number}{}", self.currency)
        };
        if neg {
            format!("-{with_curr}")
        } else {
            with_curr
        }
    }
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(char::from(b));
    }
    out
}

/// Standard `AFRange_Validate` bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AfRangeValidate {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl AfRangeValidate {
    /// Validate `value`, returning the user-facing error message when out of bounds.
    pub fn validate(&self, value: f64) -> Result<(), String> {
        if let Some(lo) = self.min
            && value < lo
        {
            return Err(format!(
                "value {value} must be greater than or equal to {lo}"
            ));
        }
        if let Some(hi) = self.max
            && value > hi
        {
            return Err(format!("value {value} must be less than or equal to {hi}"));
        }
        Ok(())
    }
}

/// Parse an `AFSimple_Calculate("OP", new Array("a", "b"))` script string.
#[must_use]
pub fn parse_af_calculate(script: &str) -> Option<(AfCalcOp, Vec<String>)> {
    let at = script.find("AFSimple_Calculate")?;
    let rest = script.get(at..)?;
    let mut quoted = Vec::new();
    let mut cur = rest;
    while let Some(q1) = cur.find('"') {
        let after_q1 = cur.get(q1 + 1..)?;
        let q2 = after_q1.find('"')?;
        quoted.push(after_q1.get(..q2)?.to_string());
        cur = after_q1.get(q2 + 1..)?;
    }
    let (first, fields) = quoted.split_first()?;
    let op = AfCalcOp::parse(first)?;
    Some((op, fields.to_vec()))
}

/// Parse an `AFRange_Validate(bMin, min, bMax, max)` script string.
#[must_use]
pub fn parse_af_range(script: &str) -> Option<AfRangeValidate> {
    let at = script.find("AFRange_Validate")?;
    let rest = script.get(at..)?;
    let open = rest.find('(')?;
    let close = rest.find(')')?;
    let args: Vec<&str> = rest
        .get(open + 1..close)?
        .split(',')
        .map(str::trim)
        .collect();
    if args.len() < 4 {
        return None;
    }
    let has_min = *args.first()? == "true" || *args.first()? == "1";
    let min_val = args.get(1)?.parse::<f64>().ok()?;
    let has_max = *args.get(2)? == "true" || *args.get(2)? == "1";
    let max_val = args.get(3)?.parse::<f64>().ok()?;
    Some(AfRangeValidate {
        min: has_min.then_some(min_val),
        max: has_max.then_some(max_val),
    })
}

/// Parse an `AFNumber_Format(nDec, sepStyle, negStyle, currStyle, "strCurrency", bPrepend)` script.
#[must_use]
pub fn parse_af_number_format(script: &str) -> Option<AfNumberFormat> {
    let at = script.find("AFNumber_Format")?;
    let rest = script.get(at..)?;
    let open = rest.find('(')?;
    let close = rest.rfind(')')?;
    let inner = rest.get(open + 1..close)?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() < 6 {
        return None;
    }
    let decimals = parts.first()?.parse::<usize>().ok()?;
    let sep_style = parts.get(1)?.parse::<i64>().unwrap_or(0);
    let currency = parts
        .get(4)?
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    let prepend = *parts.get(5)? == "true" || *parts.get(5)? == "1";
    Some(AfNumberFormat {
        decimals,
        thousands_sep: sep_style == 0 || sep_style == 2,
        currency,
        prepend_currency: prepend,
    })
}

/// Errors returned during form interaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    /// Named field does not exist in `/AcroForm`.
    NoSuchField(String),
    /// Field is marked `ReadOnly`.
    ReadOnly(String),
    /// Value failed `AFRange_Validate`.
    ValidationFailed { field: String, message: String },
    /// Underlying save error.
    Save(SaveError),
}

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchField(name) => write!(f, "no form field named `{name}`"),
            Self::ReadOnly(name) => write!(f, "form field `{name}` is read-only"),
            Self::ValidationFailed { field, message } => {
                write!(f, "validation failed for `{field}`: {message}")
            }
            Self::Save(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FormError {}

impl From<SaveError> for FormError {
    fn from(e: SaveError) -> Self {
        Self::Save(e)
    }
}

/// XFA hybrid status of a document (`FINISH.md` S6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XfaStatus {
    /// Whether `/AcroForm` contains an `/XFA` entry.
    pub present: bool,
    /// User-facing warning banner when XFA is detected.
    pub banner: Option<String>,
}

/// Detect whether `doc` is an XFA-hybrid form.
#[must_use]
pub fn detect_xfa(doc: &Document) -> XfaStatus {
    let has_xfa = doc
        .catalog()
        .ok()
        .and_then(|c| c.as_dict().cloned())
        .and_then(|d| d.get("AcroForm").cloned())
        .and_then(|a| doc.resolve_object(&a).or(Some(a)))
        .and_then(|a| a.as_dict().cloned())
        .is_some_and(|acro| acro.get("XFA").is_some());
    if has_xfa {
        XfaStatus {
            present: true,
            banner: Some(
                "This document contains an XFA form; displaying static AcroForm fallback \
                 and preserving XFA data untouched."
                    .to_string(),
            ),
        }
    } else {
        XfaStatus {
            present: false,
            banner: None,
        }
    }
}

/// Remove the `/XFA` entry from `/AcroForm` when explicitly requested by the user (`S6.7`).
pub fn strip_xfa(doc: &Document) -> Result<Save, FormError> {
    if doc.info().encryption.encrypted {
        return Err(FormError::Save(SaveError::Encrypted));
    }
    let Some(cat_ref) = doc.catalog_ref() else {
        return Err(FormError::Save(SaveError::ContentsNotIndirect));
    };
    let cat_obj = doc
        .catalog()
        .map_err(|_| FormError::Save(SaveError::ContentsNotIndirect))?;
    let Some(mut cat_dict) = cat_obj.as_dict().cloned() else {
        return Err(FormError::Save(SaveError::ContentsNotIndirect));
    };
    let mut extra = Vec::new();
    if let Some(acro_obj) = cat_dict.get("AcroForm").cloned() {
        let acro_ref = acro_obj.as_ref_id();
        if let Some(mut acro_dict) = doc
            .resolve_object(&acro_obj)
            .or(Some(acro_obj))
            .and_then(|o| o.as_dict().cloned())
        {
            acro_dict.remove("XFA");
            if let Some(r) = acro_ref {
                extra.push((r.num, Object::Dict(acro_dict)));
            } else {
                cat_dict.set("AcroForm", Object::Dict(acro_dict));
                extra.push((cat_ref.num, Object::Dict(cat_dict)));
            }
        }
    }
    Ok(write_incremental(doc, &extra))
}

/// Read all current field values (`full_name -> string_value`) from `doc`.
#[must_use]
pub fn field_values(doc: &Document) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for field in collect_fields(doc) {
        out.insert(field.full_name, field.value);
    }
    out
}

/// Fill a single form field by its hierarchical name, regenerate its appearance stream,
/// recompute `/CO` calculated fields, and set `/NeedAppearances false`.
pub fn fill_field(doc: &Document, field_name: &str, value: &str) -> Result<Save, FormError> {
    if doc.info().encryption.encrypted {
        return Err(FormError::Save(SaveError::Encrypted));
    }
    let fields = collect_fields(doc);
    let Some(target) = fields.iter().find(|f| f.full_name == field_name) else {
        return Err(FormError::NoSuchField(field_name.to_string()));
    };
    if (target.flags & FF_READ_ONLY) != 0 {
        return Err(FormError::ReadOnly(field_name.to_string()));
    }
    if let Some(range) = &target.validate
        && let Ok(num) = value.trim().parse::<f64>()
        && let Err(message) = range.validate(num)
    {
        return Err(FormError::ValidationFailed {
            field: field_name.to_string(),
            message,
        });
    }

    let final_value = if let Some(max_len) = target.max_len {
        value.chars().take(max_len).collect::<String>()
    } else {
        value.to_string()
    };

    let mut extra = Vec::new();
    let updated_target = write_field_value(doc, target, &final_value, &mut extra);
    extra.push((target.obj_ref.num, Object::Dict(updated_target)));

    let mut current_vals: BTreeMap<String, String> = fields
        .iter()
        .map(|f| (f.full_name.clone(), f.value.clone()))
        .collect();
    current_vals.insert(field_name.to_string(), final_value);

    // Evaluate `/CO` calculated fields in order.
    for f in &fields {
        if let Some((op, deps)) = &f.calc {
            let nums: Vec<f64> = deps
                .iter()
                .filter_map(|name| current_vals.get(name))
                .filter_map(|v| {
                    v.chars()
                        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                        .collect::<String>()
                        .parse::<f64>()
                        .ok()
                })
                .collect();
            let raw_val = op.evaluate(&nums);
            let formatted = match &f.format {
                Some(fmt) => fmt.format(raw_val),
                None => fmt_num(raw_val),
            };
            current_vals.insert(f.full_name.clone(), formatted.clone());
            let updated_calc = write_field_value(doc, f, &formatted, &mut extra);
            extra.push((f.obj_ref.num, Object::Dict(updated_calc)));
        }
    }

    clear_need_appearances(doc, &mut extra);
    Ok(write_incremental(doc, &extra))
}

/// Reset all form fields to their `/DV` default value (or empty if no `/DV` is present).
pub fn reset_form(doc: &Document) -> Result<Save, FormError> {
    if doc.info().encryption.encrypted {
        return Err(FormError::Save(SaveError::Encrypted));
    }
    let fields = collect_fields(doc);
    let mut extra = Vec::new();
    for f in &fields {
        let dv = f.default_value.clone().unwrap_or_default();
        let updated = write_field_value(doc, f, &dv, &mut extra);
        extra.push((f.obj_ref.num, Object::Dict(updated)));
    }
    clear_need_appearances(doc, &mut extra);
    Ok(write_incremental(doc, &extra))
}

/// Clear all form fields to empty / `/Off`.
pub fn clear_form(doc: &Document) -> Result<Save, FormError> {
    if doc.info().encryption.encrypted {
        return Err(FormError::Save(SaveError::Encrypted));
    }
    let fields = collect_fields(doc);
    let mut extra = Vec::new();
    for f in &fields {
        let empty = if f.field_type == "Btn" { "Off" } else { "" };
        let updated = write_field_value(doc, f, empty, &mut extra);
        extra.push((f.obj_ref.num, Object::Dict(updated)));
    }
    clear_need_appearances(doc, &mut extra);
    Ok(write_incremental(doc, &extra))
}

/// Export all form field values to XFDF XML (`S6.4`).
#[must_use]
pub fn export_form_xfdf(doc: &Document) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n\
         <fields>\n",
    );
    for (name, val) in field_values(doc) {
        let _ = writeln!(out, "  <field name=\"{name}\"><value>{val}</value></field>");
    }
    out.push_str("</fields>\n</xfdf>\n");
    out
}

/// Import form field values from an XFDF string (`S6.4`).
pub fn import_form_xfdf(doc: &Document, xfdf: &str) -> Result<Save, FormError> {
    let mut current_bytes = doc.bytes().to_vec();
    for line in xfdf.lines() {
        let line = line.trim();
        let Some(name_start) = line.find("name=\"") else {
            continue;
        };
        let after_name = &line[name_start + 6..];
        let Some(name_end) = after_name.find('"') else {
            continue;
        };
        let name = &after_name[..name_end];
        let Some(val_start) = line.find("<value>") else {
            continue;
        };
        let after_val = &line[val_start + 7..];
        let Some(val_end) = after_val.find("</value>") else {
            continue;
        };
        let val = &after_val[..val_end];
        let cur_doc = Document::open(current_bytes.clone(), mangle_syntax::OpenOptions::default())
            .map_err(|_| FormError::Save(SaveError::ContentsNotIndirect))?;
        let saved = fill_field(&cur_doc, name, val)?;
        current_bytes = saved.bytes;
    }
    Ok(Save {
        bytes: current_bytes,
        rewritten: Vec::new(),
        installed: Vec::new(),
        re_encoded: false,
    })
}

/// Kind of form field to create in Prepare Form mode (`S6.5`).
#[derive(Debug, Clone, PartialEq)]
pub enum NewFieldKind {
    Text {
        multiline: bool,
        password: bool,
        comb_len: Option<usize>,
    },
    CheckBox {
        checked: bool,
    },
    ComboBox {
        options: Vec<String>,
        editable: bool,
    },
    ListBox {
        options: Vec<String>,
        multi_select: bool,
    },
    PushButton {
        caption: String,
    },
    Signature,
}

/// Specification of a new form field for Prepare Form (`S6.5`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewFieldSpec {
    pub name: String,
    pub tooltip: String,
    pub rect: PageRect,
    pub default_value: String,
    pub required: bool,
    pub read_only: bool,
    pub font_size: f64,
    pub align: i64,
    pub kind: NewFieldKind,
    pub calc_script: Option<String>,
    pub format_script: Option<String>,
    pub validate_script: Option<String>,
}

impl NewFieldSpec {
    #[must_use]
    pub fn new(name: &str, rect: PageRect, kind: NewFieldKind) -> Self {
        Self {
            name: name.to_string(),
            tooltip: String::new(),
            rect,
            default_value: String::new(),
            required: false,
            read_only: false,
            font_size: 12.0,
            align: 0,
            kind,
            calc_script: None,
            format_script: None,
            validate_script: None,
        }
    }
}

/// Create a new interactive form field widget on `page` and register it in `/AcroForm` (`S6.5`).
pub fn create_field(doc: &Document, page: &Page, spec: &NewFieldSpec) -> Result<Save, FormError> {
    if doc.info().encryption.encrypted {
        return Err(FormError::Save(SaveError::Encrypted));
    }
    let Some(page_ref) = page.object_ref() else {
        return Err(FormError::Save(SaveError::ContentsNotIndirect));
    };
    let Some(cat_ref) = doc.catalog_ref() else {
        return Err(FormError::Save(SaveError::ContentsNotIndirect));
    };
    let cat_obj = doc
        .catalog()
        .map_err(|_| FormError::Save(SaveError::ContentsNotIndirect))?;
    let Some(mut cat_dict) = cat_obj.as_dict().cloned() else {
        return Err(FormError::Save(SaveError::ContentsNotIndirect));
    };

    let mut extra = Vec::new();
    let field_num = doc.alloc().num;
    let field_ref = Ref::new(field_num, 0);

    let mut flags = 0i64;
    if spec.read_only {
        flags |= FF_READ_ONLY;
    }
    if spec.required {
        flags |= FF_REQUIRED;
    }

    let mut d = Dict::new();
    d.set("Type", Object::name("Annot"));
    d.set("Subtype", Object::name("Widget"));
    d.set("T", Object::string(&spec.name));
    if !spec.tooltip.is_empty() {
        d.set("TU", Object::string(&spec.tooltip));
    }
    d.set(
        "Rect",
        Object::Array(vec![
            Object::Real(spec.rect.left),
            Object::Real(spec.rect.bottom),
            Object::Real(spec.rect.right),
            Object::Real(spec.rect.top),
        ]),
    );
    d.set("F", Object::Int(4));
    d.set("Q", Object::Int(spec.align));
    d.set(
        "DA",
        Object::string(&format!("/F1 {} Tf 0 0 0 rg", fmt_num(spec.font_size))),
    );
    if !spec.default_value.is_empty() {
        d.set("DV", Object::string(&spec.default_value));
        d.set("V", Object::string(&spec.default_value));
    }

    match &spec.kind {
        NewFieldKind::Text {
            multiline,
            password,
            comb_len,
        } => {
            d.set("FT", Object::name("Tx"));
            if *multiline {
                flags |= FF_MULTILINE;
            }
            if *password {
                flags |= FF_PASSWORD;
            }
            if let Some(len) = comb_len {
                flags |= FF_COMB;
                d.set("MaxLen", Object::Int(*len as i64));
            }
        }
        NewFieldKind::CheckBox { checked } => {
            d.set("FT", Object::name("Btn"));
            let state = if *checked { "Yes" } else { "Off" };
            d.set("V", Object::name(state));
            d.set("AS", Object::name(state));
        }
        NewFieldKind::ComboBox { options, editable } => {
            d.set("FT", Object::name("Ch"));
            flags |= FF_COMBO;
            if *editable {
                flags |= FF_EDIT;
            }
            d.set(
                "Opt",
                Object::Array(options.iter().map(|o| Object::string(o)).collect()),
            );
        }
        NewFieldKind::ListBox {
            options,
            multi_select,
        } => {
            d.set("FT", Object::name("Ch"));
            if *multi_select {
                flags |= FF_MULTI_SELECT;
            }
            d.set(
                "Opt",
                Object::Array(options.iter().map(|o| Object::string(o)).collect()),
            );
        }
        NewFieldKind::PushButton { caption } => {
            d.set("FT", Object::name("Btn"));
            flags |= FF_PUSHBUTTON;
            let mut mk = Dict::new();
            mk.set("CA", Object::string(caption));
            d.set("MK", Object::Dict(mk));
        }
        NewFieldKind::Signature => {
            d.set("FT", Object::name("Sig"));
        }
    }
    d.set("Ff", Object::Int(flags));

    let mut aa = Dict::new();
    if let Some(js) = &spec.calc_script {
        aa.set("C", js_action(js));
    }
    if let Some(js) = &spec.format_script {
        aa.set("F", js_action(js));
    }
    if let Some(js) = &spec.validate_script {
        aa.set("V", js_action(js));
    }
    if !aa.is_empty() {
        d.set("AA", Object::Dict(aa));
    }

    let ap_stream = build_widget_appearance(
        doc,
        spec.rect,
        &spec.default_value,
        spec.font_size,
        flags,
        None,
        &mut extra,
    );
    let ap_num = doc.alloc().num;
    extra.push((ap_num, Object::Stream(ap_stream)));
    let mut ap_dict = Dict::new();
    ap_dict.set("N", Object::Ref(Ref::new(ap_num, 0)));
    d.set("AP", Object::Dict(ap_dict));

    extra.push((field_num, Object::Dict(d)));

    // Append to page `/Annots`.
    let mut annots = page
        .dict
        .get("Annots")
        .and_then(|a| doc.resolve_object(a).or_else(|| Some(a.clone())))
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    annots.push(Object::Ref(field_ref));
    let mut page_dict = page.dict.clone();
    page_dict.set("Annots", Object::Array(annots));
    extra.push((page_ref.num, Object::Dict(page_dict)));

    // Register in catalog `/AcroForm`.
    let acro_obj = cat_dict.get("AcroForm").cloned();
    let acro_ref = acro_obj.as_ref().and_then(Object::as_ref_id);
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
    acro_dict.set("NeedAppearances", Object::Bool(false));
    if spec.calc_script.is_some() {
        let mut co = acro_dict
            .get("CO")
            .and_then(|c| doc.resolve_object(c).or_else(|| Some(c.clone())))
            .and_then(|c| c.as_array().map(<[Object]>::to_vec))
            .unwrap_or_default();
        co.push(Object::Ref(field_ref));
        acro_dict.set("CO", Object::Array(co));
    }
    if let Some(r) = acro_ref {
        extra.push((r.num, Object::Dict(acro_dict)));
    } else {
        cat_dict.set("AcroForm", Object::Dict(acro_dict));
        extra.push((cat_ref.num, Object::Dict(cat_dict)));
    }

    Ok(write_incremental(doc, &extra))
}

fn js_action(js: &str) -> Object {
    let mut act = Dict::new();
    act.set("S", Object::name("JavaScript"));
    act.set("JS", Object::string(js));
    Object::Dict(act)
}

#[derive(Debug, Clone)]
struct ResolvedField {
    obj_ref: Ref,
    dict: Dict,
    full_name: String,
    field_type: String,
    flags: i64,
    max_len: Option<usize>,
    value: String,
    default_value: Option<String>,
    rect: PageRect,
    font_size: f64,
    calc: Option<(AfCalcOp, Vec<String>)>,
    format: Option<AfNumberFormat>,
    validate: Option<AfRangeValidate>,
}

fn collect_fields(doc: &Document) -> Vec<ResolvedField> {
    let Ok(cat) = doc.catalog() else {
        return Vec::new();
    };
    let Some(cat_dict) = cat.as_dict() else {
        return Vec::new();
    };
    let Some(acro) = cat_dict
        .get("AcroForm")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
    else {
        return Vec::new();
    };
    let Some(roots) = acro
        .get("Fields")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in &roots {
        walk_field(doc, entry, "", None, 0, &mut out, 0);
    }
    out
}

fn walk_field(
    doc: &Document,
    entry: &Object,
    prefix: &str,
    inherited_ft: Option<&str>,
    inherited_ff: i64,
    out: &mut Vec<ResolvedField>,
    depth: usize,
) {
    if depth > 16 {
        return;
    }
    let Some(obj_ref) = entry.as_ref_id() else {
        return;
    };
    let Some(resolved) = doc.resolve_object(entry) else {
        return;
    };
    let Some(dict) = resolved.as_dict().cloned() else {
        return;
    };
    let partial = dict
        .get("T")
        .and_then(Object::as_bytes)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let full_name = if prefix.is_empty() {
        partial
    } else if partial.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}.{partial}")
    };
    let ft = dict
        .get("FT")
        .and_then(Object::as_name)
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .or_else(|| inherited_ft.map(str::to_string));
    let ff = dict
        .get("Ff")
        .and_then(Object::as_i64)
        .unwrap_or(inherited_ff);

    if let Some(kids) = dict
        .get("Kids")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_array().map(<[Object]>::to_vec))
    {
        for kid in &kids {
            walk_field(doc, kid, &full_name, ft.as_deref(), ff, out, depth + 1);
        }
        return;
    }

    let value = dict
        .get("V")
        .and_then(|v| {
            v.as_bytes()
                .or_else(|| v.as_name())
                .map(|b| String::from_utf8_lossy(b).into_owned())
        })
        .unwrap_or_default();
    let default_value = dict.get("DV").and_then(|v| {
        v.as_bytes()
            .or_else(|| v.as_name())
            .map(|b| String::from_utf8_lossy(b).into_owned())
    });
    let max_len = dict
        .get("MaxLen")
        .and_then(Object::as_i64)
        .and_then(|n| usize::try_from(n).ok());
    let rect = dict
        .get("Rect")
        .and_then(|r| {
            PageRect::from_object(&doc.resolve_object(r).unwrap_or_else(|| r.clone())).ok()
        })
        .unwrap_or_else(|| PageRect::new(0.0, 0.0, 100.0, 20.0));

    let (calc, format, validate) = parse_aa_scripts(doc, &dict);

    out.push(ResolvedField {
        obj_ref,
        dict,
        full_name,
        field_type: ft.unwrap_or_else(|| "Tx".to_string()),
        flags: ff,
        max_len,
        value,
        default_value,
        rect,
        font_size: 12.0,
        calc,
        format,
        validate,
    });
}

type AaScripts = (
    Option<(AfCalcOp, Vec<String>)>,
    Option<AfNumberFormat>,
    Option<AfRangeValidate>,
);

fn parse_aa_scripts(doc: &Document, dict: &Dict) -> AaScripts {
    let Some(aa) = dict
        .get("AA")
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
    else {
        return (None, None, None);
    };
    let js_of = |key: &str| -> Option<String> {
        let act = aa
            .get(key)
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))?;
        let d = act.as_dict()?;
        let js = d
            .get("JS")
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))?;
        js.as_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    };
    let calc = js_of("C").and_then(|s| parse_af_calculate(&s));
    let format = js_of("F").and_then(|s| parse_af_number_format(&s));
    let validate = js_of("V").and_then(|s| parse_af_range(&s));
    (calc, format, validate)
}

fn write_field_value(
    doc: &Document,
    field: &ResolvedField,
    value: &str,
    extra: &mut Vec<(u32, Object)>,
) -> Dict {
    let mut d = field.dict.clone();
    if field.field_type == "Btn" {
        let state = if value.is_empty() { "Off" } else { value };
        d.set("V", Object::name(state));
        d.set("AS", Object::name(state));
    } else {
        d.set("V", Object::string(value));
    }
    let ap_stream = build_widget_appearance(
        doc,
        field.rect,
        value,
        field.font_size,
        field.flags,
        field.max_len,
        extra,
    );
    let ap_num = doc.alloc().num;
    extra.push((ap_num, Object::Stream(ap_stream)));
    let mut ap_dict = Dict::new();
    ap_dict.set("N", Object::Ref(Ref::new(ap_num, 0)));
    d.set("AP", Object::Dict(ap_dict));
    d
}

fn build_widget_appearance(
    doc: &Document,
    rect: PageRect,
    value: &str,
    font_size: f64,
    flags: i64,
    max_len: Option<usize>,
    extra: &mut Vec<(u32, Object)>,
) -> Stream {
    let mut dict = Dict::new();
    dict.set("Type", Object::name("XObject"));
    dict.set("Subtype", Object::name("Form"));
    dict.set(
        "BBox",
        Object::Array(vec![
            Object::Real(rect.left),
            Object::Real(rect.bottom),
            Object::Real(rect.right),
            Object::Real(rect.top),
        ]),
    );

    let shown = if (flags & FF_PASSWORD) != 0 {
        "*".repeat(value.chars().count())
    } else {
        value.to_string()
    };

    let mut res = Dict::new();
    let mut font_table = Dict::new();
    let mut ops = String::from("q 0 0 0 rg ");

    let chars: Vec<char> = shown.chars().collect();
    let needs_unicode = chars.iter().any(|c| !c.is_ascii());
    if needs_unicode && let Some((family, program)) = mangle_font::substitute_named("Helvetica") {
        let first = doc.alloc().num;
        for _ in 1..EMBEDDED_OBJECTS {
            doc.alloc();
        }
        if let Ok(embedded) = embed_truetype(program, &chars, family, first) {
            font_table.set("F1", Object::Ref(Ref::new(embedded.type0, 0)));
            for (num, obj) in embedded.numbered {
                extra.push((num, obj));
            }
            let mut raw = Vec::new();
            for ch in &chars {
                if let Some((_, cid)) = embedded.cids.iter().find(|(c, _)| c == ch) {
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
                fmt_num(font_size),
                fmt_num(rect.left + 2.0),
                fmt_num(rect.bottom + 4.0),
            );
        }
    } else {
        let mut f1 = Dict::new();
        f1.set("Type", Object::name("Font"));
        f1.set("Subtype", Object::name("Type1"));
        f1.set("BaseFont", Object::name("Helvetica"));
        f1.set("Encoding", Object::name("WinAnsiEncoding"));
        font_table.set("F1", Object::Dict(f1));

        if (flags & FF_COMB) != 0
            && let Some(cells) = max_len
            && cells > 0
        {
            let cell_w = rect.width() / (cells as f64);
            for (idx, ch) in shown.chars().take(cells).enumerate() {
                let mut lit = Vec::new();
                let mut buf = [0u8; 4];
                mangle_syntax::writer::write_literal_string(
                    &mut lit,
                    ch.encode_utf8(&mut buf).as_bytes(),
                );
                let lit_s = String::from_utf8_lossy(&lit);
                let x = rect.left + (idx as f64 + 0.25) * cell_w;
                let _ = write!(
                    ops,
                    "BT /F1 {} Tf {} {} Td {lit_s} Tj ET ",
                    fmt_num(font_size),
                    fmt_num(x),
                    fmt_num(rect.bottom + 4.0),
                );
            }
        } else {
            let mut lit = Vec::new();
            mangle_syntax::writer::write_literal_string(&mut lit, shown.as_bytes());
            let lit_s = String::from_utf8_lossy(&lit);
            let _ = write!(
                ops,
                "BT /F1 {} Tf {} {} Td {lit_s} Tj ET ",
                fmt_num(font_size),
                fmt_num(rect.left + 2.0),
                fmt_num(rect.bottom + 4.0),
            );
        }
    }

    ops.push('Q');
    res.set("Font", Object::Dict(font_table));
    dict.set("Resources", Object::Dict(res));
    Stream::new(dict, ops.into_bytes())
}

fn clear_need_appearances(doc: &Document, extra: &mut Vec<(u32, Object)>) {
    let Some(cat_ref) = doc.catalog_ref() else {
        return;
    };
    let Ok(cat_obj) = doc.catalog() else {
        return;
    };
    let Some(mut cat_dict) = cat_obj.as_dict().cloned() else {
        return;
    };
    let Some(acro_obj) = cat_dict.get("AcroForm").cloned() else {
        return;
    };
    let acro_ref = acro_obj.as_ref_id();
    let Some(mut acro_dict) = doc
        .resolve_object(&acro_obj)
        .or(Some(acro_obj))
        .and_then(|o| o.as_dict().cloned())
    else {
        return;
    };
    acro_dict.set("NeedAppearances", Object::Bool(false));
    if let Some(r) = acro_ref {
        extra.push((r.num, Object::Dict(acro_dict)));
    } else {
        cat_dict.set("AcroForm", Object::Dict(acro_dict));
        extra.push((cat_ref.num, Object::Dict(cat_dict)));
    }
}

fn write_incremental(doc: &Document, extra: &[(u32, Object)]) -> Save {
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

    use mangle_doc::PageTree;
    use mangle_syntax::object::Rect as PageRect;
    use mangle_syntax::{Document, OpenOptions};

    use super::{
        FormError, NewFieldKind, NewFieldSpec, clear_form, create_field, detect_xfa,
        export_form_xfdf, field_values, fill_field, import_form_xfdf, reset_form, strip_xfa,
    };

    fn blank_form_pdf(with_xfa: bool) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        if with_xfa {
            out.push_str(
                "1 0 obj\n<</Type /Catalog /Pages 2 0 R \
                 /AcroForm <</Fields [] /NeedAppearances true /XFA (xfa-payload)>>>>\nendobj\n",
            );
        } else {
            out.push_str(
                "1 0 obj\n<</Type /Catalog /Pages 2 0 R \
                 /AcroForm <</Fields [] /NeedAppearances true>>>>\nendobj\n",
            );
        }
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

    fn open_doc(bytes: Vec<u8>) -> (Document, mangle_doc::Page) {
        let doc = Document::open(bytes, OpenOptions::default()).expect("opens");
        let cat = doc.catalog().expect("cat");
        let root = cat
            .get("Pages")
            .and_then(mangle_syntax::object::Object::as_ref_id)
            .expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        let page = tree.pages().first().expect("page").clone();
        (doc, page)
    }

    #[test]
    fn prepare_fill_calculate_format_validate_and_xfdf_round_trip() {
        let (doc, page) = open_doc(blank_form_pdf(false));

        // 1. Create price1 (with range validation 0..=1000)
        let mut p1 = NewFieldSpec::new(
            "price1",
            PageRect::new(20.0, 150.0, 100.0, 170.0),
            NewFieldKind::Text {
                multiline: false,
                password: false,
                comb_len: None,
            },
        );
        p1.default_value = "10".to_string();
        p1.validate_script = Some("AFRange_Validate(true, 0, true, 1000)".to_string());
        let s1 = create_field(&doc, &page, &p1).expect("creates price1");

        // 2. Create price2
        let (doc2, page2) = open_doc(s1.bytes);
        let mut p2 = NewFieldSpec::new(
            "price2",
            PageRect::new(20.0, 120.0, 100.0, 140.0),
            NewFieldKind::Text {
                multiline: false,
                password: false,
                comb_len: Some(4),
            },
        );
        p2.default_value = "25".to_string();
        let s2 = create_field(&doc2, &page2, &p2).expect("creates price2");

        // 3. Create total with AFSimple_Calculate("SUM", ...) and AFNumber_Format
        let (doc3, page3) = open_doc(s2.bytes);
        let mut total = NewFieldSpec::new(
            "total",
            PageRect::new(20.0, 90.0, 120.0, 110.0),
            NewFieldKind::Text {
                multiline: false,
                password: false,
                comb_len: None,
            },
        );
        total.calc_script =
            Some("AFSimple_Calculate(\"SUM\", new Array(\"price1\", \"price2\"))".to_string());
        total.format_script = Some("AFNumber_Format(2, 0, 0, 0, \"$\", true)".to_string());
        let s3 = create_field(&doc3, &page3, &total).expect("creates total");

        let (doc4, _) = open_doc(s3.bytes);
        // Validation rejects out-of-range value:
        let err = fill_field(&doc4, "price1", "2500").expect_err("out of range");
        assert!(matches!(err, FormError::ValidationFailed { .. }));

        // Valid fill updates `price1` and recalculates `total` as "$1,025.00"!
        let filled = fill_field(&doc4, "price1", "1000").expect("fills price1");
        let (doc5, _) = open_doc(filled.bytes);
        let vals = field_values(&doc5);
        assert_eq!(vals.get("price1").map(String::as_str), Some("1000"));
        assert_eq!(vals.get("total").map(String::as_str), Some("$1,025.00"));

        // XFDF export and import into a fresh copy, plus reset and clear:
        let xfdf = export_form_xfdf(&doc5);
        assert!(xfdf.contains("$1,025.00"));

        let reset = reset_form(&doc5).expect("resets form");
        let (doc_reset, _) = open_doc(reset.bytes);
        assert_eq!(
            field_values(&doc_reset).get("price1").map(String::as_str),
            Some("10")
        );

        let cleared = clear_form(&doc_reset).expect("clears form");
        let (doc_clear, _) = open_doc(cleared.bytes);
        assert_eq!(
            field_values(&doc_clear).get("price1").map(String::as_str),
            Some("")
        );

        let reimported = import_form_xfdf(&doc_clear, &xfdf).expect("imports xfdf");
        let (doc_imp, _) = open_doc(reimported.bytes);
        assert_eq!(
            field_values(&doc_imp).get("price1").map(String::as_str),
            Some("1000")
        );
    }

    #[test]
    fn unicode_form_input_and_xfa_detection_and_strip() {
        let (doc, page) = open_doc(blank_form_pdf(true));
        let status = detect_xfa(&doc);
        assert!(status.present);
        assert!(status.banner.is_some());

        let spec = NewFieldSpec::new(
            "guest",
            PageRect::new(20.0, 150.0, 160.0, 170.0),
            NewFieldKind::Text {
                multiline: false,
                password: false,
                comb_len: None,
            },
        );
        let created = create_field(&doc, &page, &spec).expect("creates guest");
        let (doc2, _) = open_doc(created.bytes);
        let filled = fill_field(&doc2, "guest", "Зоя — Ζωή").expect("unicode fill");
        let (doc3, _) = open_doc(filled.bytes);
        assert_eq!(
            field_values(&doc3).get("guest").map(String::as_str),
            Some("Зоя — Ζωή")
        );
        // XFA preserved after form fill:
        assert!(detect_xfa(&doc3).present);

        // Strip XFA removes it cleanly:
        let stripped = strip_xfa(&doc3).expect("strips xfa");
        let (doc4, _) = open_doc(stripped.bytes);
        assert!(!detect_xfa(&doc4).present);
    }
}
