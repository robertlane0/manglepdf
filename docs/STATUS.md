# Status

Living dashboard of milestone progress, existing capabilities, test coverage, and open gaps.
Detailed per-defect corpus measurements live in [`docs/known-diffs.md`](known-diffs.md); closed
write-back bugs live in [`docs/BUGS.md`](BUGS.md).

## Milestones

| M | Deliverable | State |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen, window shell, icon pipeline | **done** — every Gate 0 check passes; 129 hand-authored SVG icons and light/dark tokens in place |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector | **mostly done** — syntax, recovery, decryption, and writers complete; Developer Inspector UI pending |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | **mostly done** — paths, per-pixel region clips, TrueType/CFF/Type 1/composite glyphs, images, shadings, and Form XObjects render; sidebar thumbnail click/bookmarks/layers UI wiring pending |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | **substantially done** — all 4 PDF function types, axial/radial shadings, all 16 PDF blend modes, Device/Cal/ICCBased/Indexed/Separation/DeviceN/Pattern colour spaces, `PaintType` 1 & 2 tiling patterns, TrueType/CFF/Type 1/Type 3/composite fonts, `/ToUnicode` CMaps, and 12 bundled Standard-14 substitute faces work; OCG visibility toggling in the UI remains |
| **M4** | Page objects, select/move/scale/recolour, undo/redo, first save→reopen | **done** — `PageModel`, byte-range `surgery`, `edits` (move, scale, rotate, delete, recolour, crop, text state), `arrange` with state re-materialisation, `History`, incremental `writeback`, `Editor` session, and worker-driven GUI editing complete |
| **M5** | Text model, editing, reflow, font resolution/embedding/`ToUnicode`, Text Properties panel | **done at engine level** — text extraction (simple + `/Type0` via `/ToUnicode`) and search (`mangle-text`) work; surgical retyping works across rung 1 (`Change::Retype` in own simple or `/Type0` font), rung 2 (`Change::RetypeFace` in same-family document font), own-`/FontFile2` subset re-embedding, and rung 3 (`Change::RetypeIn` via TrueType subsetting + 5-object CID embedding + `ToUnicode` + status-bar notice), plus Type 3 replacement (`replace_with_live_text`), paragraph reflow with alignment (`mangle-edit::reflow`), and shared Form XObject copy-on-write; UI font/colour pickers and `--font-dirs` remain |
| **M6** | Annotations, add content, flatten | **substantially done** — `mangle-edit::annotations` (deterministic `/AP /N` generation for Highlight/Underline/StrikeOut/Squiggly, StickyNote + Popup + `/IRT` replies, Unicode FreeText, Line/Square/Circle/Polygon/PolyLine/Ink/Stamp/Caret/Link/Redact; XFDF import/export; Header/Footer and Watermark artifacts) and `mangle-edit::flatten` (pixel-equivalent, idempotent flattening for `All`, `MarkupOnly`, `FormsOnly`, single index, `Hidden`/`NoView` skipping, and signed-document confirmation) are implemented and tested; UI Annotate panel wiring remains |
| **M7** | Forms: fill, calculation/format/validate, Prepare Form, XFDF, XFA | **substantially done** — `mangle-edit::forms` implements field filling (`Tx`, `Btn`, `Ch`, `Sig`), native `AFSimple_Calculate` (`SUM`/`PRD`/`AVG`/`MIN`/`MAX`), `AFNumber_Format`, `AFRange_Validate`, Prepare Form field creation (`create_field`), XFDF form data export/import, `reset_form`/`clear_form`, Unicode subset embedding in widget `/AP /N`, and XFA detection/strip |
| **M8** | Organize pages: reorder, insert, delete, rotate, crop, extract, split, merge, replace, labels | **substantially done** — `mangle-edit::organize` implements `reorder_page`, `rotate_page`, `crop_page`, `insert_blank_page`, `insert_pages_from_doc` (with SHA-256 stream deduplication), `delete_pages` (with full rewrite + GC), `extract_pages`, `split_every_n`, `merge_documents`, `replace_page`, `set_page_labels` (plus `/Nums` tree support in `mangle-doc::PageLabel`), and stale `/Thumb` removal |
| **M9** | Protect: redaction, leak scan, sanitize, security removal, digital signatures | **substantially done** — `mangle-edit::protect` implements `mark_redaction_rect`, `apply_redactions` (irreversible token/stream scrub + opaque box painting + forced full rewrite), `leak_scan` (raw bytes + all decoded streams), `sanitize_document`, `remove_security`, `sign_document` (detached SHA-256 CMS with exact `/ByteRange` and DocMDP), and `verify_signatures` (tamper & post-signature revision detection) |
| **M10** | Export, image extraction, PDF creation from images, optimize | **substantially done** — `mangle-edit::export` implements in-house `encode_png`, `export_page_png`, `export_page_jpeg`, `extract_images` (preserving JPEG streams byte-for-byte), `create_pdf_from_images` (JPEG passthrough + RGBA `/SMask`), and deterministic `optimize_document` (`Lossless` and `Small` presets with stream compression and SHA-256 deduplication); `mangle-cli render` wired |
| **M11**–**M12** | UI polish, complete Tier-A fixtures, gauntlet | **in progress** |

## What exists by crate

- **`xtask` & `tools/fixturegen`** — `cargo xtask policy` enforces Gate 0 (edition 2024, AST `unsafe` scan, dependency audit against `docs/DEPENDENCIES.md`, process-spawn audit, fmt/clippy/test/release build, panic-free library lints, asset licences, SVG icon lint, doc presence and anchor link verification, and `fixturegen` independence). `tools/fixturegen` deterministically builds 12 Tier-A fixtures without depending on any `mangle-*` crate.
- **`mangle-filters`** — In-house zlib/deflate/inflate, LZW (`EarlyChange` 0/1), RunLength, ASCII85, ASCIIHex, PNG (10–15) and TIFF (2) predictors, and CCITT G3-1D/2D & G4 (verified byte-for-byte against `libtiff`). Returns partial output with diagnostic notes on truncated/corrupt input.
- **`mangle-crypto`** — RC4, AES-CBC, MD5, SHA-1/2, SASLprep, and Standard Security Handler R2–R6 decryption and permission bit parsing.
- **`mangle-syntax`** — Lossless COS object graph, lexer, parser, classic xref tables, xref streams (`/Index`, `/W`, `/Prev`, hybrid `/XRefStm`), object streams (`/Extends`), fallback scanner recovery, copy-on-write document overlay, full writer, and incremental revision writer.
- **`mangle-doc`** — Catalog, page tree with cycle detection and attribute inheritance, name/number trees, outlines/bookmarks, destinations, page labels, optional-content groups, attachments, and metadata/XMP.
- **`mangle-font` & `assets/fonts/`** — Verified Standard-14 width tables and metric-compatible aliases (`Arial`, `TimesNewRoman`, `CourierNew`), `/Widths`/`/MissingWidth`, CID `/W`/`/DW` and `/CIDToGIDMap`, Standard/WinAnsi/MacRoman/PDFDoc encodings + `/Differences` + 912-entry AGL, TrueType/CFF/Type 1 outline extraction, 12 bundled OFL Liberation substitute faces, TrueType font subsetting (`subset_truetype`), and the `--font-dirs` fallback resolver (`discover_font_files`, `inspect_font_file`, `find_system_font`, `resolve_fallback` with system-first order; call-site threading remains).
- **`mangle-content`** — Provenance-preserving tokeniser and operator table, graphics and text state, content stream interpreter, Form XObject execution (`MAX_FORM_DEPTH = 12`), and all 4 PDF function types.
- **`mangle-render`** — Analytic scanline coverage rasterizer, per-pixel region clipping masks, Device/Cal/ICCBased/Indexed/Separation/DeviceN/Pattern colour spaces, axial/radial shadings, tiled patterns (`PaintType` 1 & 2), images (`/Decode`, `/ImageMask`, `/SMask` with pre-allocation memory bounds), annotation `/AP /N` rendering, in-house SVG icon renderer, and SSIM/RMS/heatmap image comparison.
- **`mangle-text`** — Decodes glyph runs to Unicode strings via font encodings/AGL and `ToUnicode`, joins runs into lines and words using baseline and em-scaled gap thresholds, and provides case-insensitive search (`find`) with byte offsets and object attribution.
- **`mangle-edit`** — `PageModel` grouping, byte-range `surgery` with `q`/`Q` balance verification, `edits` (move, scale, rotate, delete, recolour, crop, text state properties, and rungs 1–3 of text retyping + Type 3 live-text replacement), `reflow` (paragraph layout with `Left`/`Center`/`Right`/`Justify`, overflow flag, WinAnsi-honest encoding, and shared Form XObject copy-on-write), `embed_truetype` (5-object CID font hierarchy with `/ToUnicode`), `arrange` z-order movement with state re-materialisation, bounded `Arc`-snapshot `History`, incremental `writeback`, and `Editor` session.
- **`mangle-ui` & `mangle-cli`** — Token-themed window shell with background `Supervisor` worker, click-to-select and drag-to-move on canvas, contextual Text Properties and Colour panels with live steppers and text content editing, Arrange and Actions (Rotate ±90°, Delete) sections, title-bar search (`Ctrl+F`) with hit navigation and canvas highlights, bottom-bar zoom (`−`/`100%`/`+`, Fit, Fullscreen) and page navigation, and headless `mangle-cli` commands (`info`, `pages`, `check`, `extract`, `save`, `edit`).

## A form XObject is a nested content stream, not an image with no samples

`Do` dispatches on `/Subtype` in the XObject dictionary: `/Image` decodes samples, while
`/Form` executes as a nested content stream via `run_with_state` (bounded by
`MAX_FORM_DEPTH = 12`) with the form's `/Matrix` composed into the CTM, `/BBox` installed as a
transformed clip, and `/Resources` resolved recursively (inheriting from the enclosing scope
when omitted). Each emitted `Record` carries the form name so `mangle-render` and `mangle-edit`
resolve font, image, and shading names against the form's own resource table. See
[D17](known-diffs.md#d17--a-cid-was-looked-up-through-the-fonts-cmap-instead-of-its-cidtogidmap)
and
[D18](known-diffs.md#d18--an-smask-was-never-checked-against-the-image-it-masks-and-a-hostile-one-cost-a-gigabyte)
for the companion CID and `/SMask` fixes uncovered on form-heavy corpus pages.

## Where the corpus stands now

The Tier-B wild corpus (`corpus/wild/`) holds 77 real-world PDFs from pdf.js, PDFBox, NIST,
IRS, USGS, and arXiv, compared at 150 DPI against `mutool draw` and `pdftotext`:

- **73 of 77 files open** without repair prompts (the remaining 4 are 2 fuzzed files `qpdf`
  also rejects and 2 encrypted files).
- **740 pages compared**, **0 panics**, page median SSIM **0.96020**, page mean SSIM **0.94307**,
  per-file worst-page median **0.97375**, and **164 pages below 0.95** (down from 254 across the
  previous three runs).
- Of the 164 pages below 0.95, 62 are JPEG 2000 images (`nistir7255`, `S2`) and ~70 are
  unembedded fonts rendered in metric-compatible Liberation substitute faces vs `mutool`'s URW
  Nimbus faces ([D24](known-diffs.md)). Full per-file breakdown is in
  [`docs/known-diffs.md`](known-diffs.md).

## Tests

Over 1,210 passing unit, round-trip, provenance, corpus, and oracle tests across 39 test
binaries, with 0 failures and 1 sanctioned `#[ignore]`d test (`the_wild_corpus_is_measured_and_reported`,
the full two-hour Tier-B corpus run).

## Open gaps (in priority order)

1. **M5 UI remains (engine done)** — Alignment control in the Text Properties panel (engine: `Change::Reflow` landed) and UI prompt wiring for shared Form XObject copy-on-write (unopened-form edits refused by name meanwhile).
2. **Stroke `closepath` state (`D14`, closed) and Type 3 glyph execution (landed)** — Open polylines keep their caps and rings keep their inner boundary (`a_closepath_is_carried_through_to_the_polygon`, `an_open_subpath_is_one_outline_and_a_ring_is_two`); Type 3 `/CharProcs` execute under `MAX_TYPE3_DEPTH = 8`.
3. **Viewer sidebar & panels (M1/M2/M3)** — Clickable thumbnail navigation, Bookmarks/Outlines
   tree navigation, OCG Layers toggle panel, Attachments/Signatures panels, and Developer
   Inspector in `mangle-ui`.
4. **M6 Annotations, Add Content & Flatten** — Annotation creation/editing, `/AP /N` generators
   for all P0 annotation types, Comments panel, FDF/XFDF round-trip, Header/Footer/Watermark/Bates
   page artifacts, and Flatten.
5. **M7 Forms** — Interactive form filling, `AF*` calculations/formatting/validation, Prepare
   Form, and form flattening.
6. **M8 Page Organization** — Reorder, insert, delete, rotate, crop, extract, split, merge,
   replace, resource deduplication, and destination retargeting.
7. **M9 Protect** — Redaction mark/search/apply with leak scan, Sanitize, encryption writing
   (R2–R6), and PKCS#7/CMS digital signatures.
8. **M10–M12 Export, Optimize, Fixtures & Gauntlet** — Raster/text/image export, PDF creation
   from images, Optimize/compress, remaining Tier-A fixtures (incl. F31 `TravelGuide.pdf`),
   `--automation-stdio` (parsed; the line protocol is still open), and the `cargo xtask gauntlet` runner.
