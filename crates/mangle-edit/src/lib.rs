//! Editing.
//!
//! A command pattern over immutable snapshots, undo and redo, surgical write-back, the
//! annotation and appearance generators, forms, redaction, flatten, page operations and
//! the optimizer. Every edit is one command and one undo step, and every write is
//! checked against the model it came from before it is saved.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

pub mod annotations;
pub mod arrange;
pub mod edits;
pub mod embed;
pub mod export;
pub mod flatten;
pub mod forms;
pub mod history;
pub mod organize;
pub mod page_objects;
pub mod protect;
pub mod session;
pub mod surgery;
pub mod writeback;

pub use annotations::{
    ANNOT_FLAG_PRINT, AddedAnnotation, AnnotationBody, AnnotationSpec, LineEnding, LinkAction,
    MarkupKind, add_annotation, add_header_footer, add_watermark, arrange_annotation,
    delete_annotation, export_xfdf, import_xfdf, normalize_quad,
};
pub use arrange::{Arrange, ArrangeError, ArrangeRefusal, apply_arrange, arrange_patches, extent};
pub use edits::{
    Change, ChangeError, Channel, Fonts, Refusal, TextProperty, apply_change, apply_change_with,
    line_spacing, map_point, patches_for, patches_for_with, text_value, verify, verify_with,
};
pub use embed::{EMBEDDED_OBJECTS, EmbedError, EmbeddedFont, embed_truetype};
pub use export::{
    ExtractedImage, ImageFormat, ImageInput, OptimizePreset, OptimizeReport, PageFit,
    create_pdf_from_images, encode_png, export_page_jpeg, export_page_png, extract_images,
    optimize_document,
};
pub use flatten::{
    FlattenError, FlattenFilter, FlattenOptions, FlattenReport, flatten_page, has_digital_signature,
};
pub use forms::{
    AfCalcOp, AfNumberFormat, AfRangeValidate, FF_COMB, FF_COMBO, FF_EDIT, FF_MULTI_SELECT,
    FF_MULTILINE, FF_NO_TOGGLE_TO_OFF, FF_PASSWORD, FF_PUSHBUTTON, FF_RADIO, FF_RADIOS_IN_UNISON,
    FF_READ_ONLY, FF_REQUIRED, FormError, NewFieldKind, NewFieldSpec, XfaStatus, clear_form,
    create_field, detect_xfa, export_form_xfdf, field_values, fill_field, import_form_xfdf,
    parse_af_calculate, parse_af_number_format, parse_af_range, reset_form, strip_xfa,
};
pub use history::{Edit, History, HistoryError, Snapshot};
pub use organize::{
    LabelStyle, PageLabelRange, crop_page, delete_pages, extract_pages, insert_blank_page,
    insert_pages_from_doc, merge_documents, reorder_page, replace_page, rotate_page,
    set_page_labels, split_every_n,
};
pub use page_objects::{
    Kind, LineBreak, PageModel, PageObject, Provenance, Summary, content, page_text, provenance_of,
    summarise,
};
pub use protect::{
    RedactionOptions, RedactionReport, SanitizeReport, SignatureSpec, SignatureStatus,
    apply_redactions, leak_scan, mark_redaction_rect, remove_security, sanitize_document,
    sign_document, verify_signatures,
};
pub use session::{Editor, InstallError, OpenError, Outcome, Reason, SessionError};
pub use surgery::{Applied, EditError, Patch};
pub use writeback::{Save, SaveError, save_decoded, save_decoded_with, save_page, stream_digest};
