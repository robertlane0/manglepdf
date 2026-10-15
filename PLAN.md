# ManglePDF — Plan

> The authoritative specification is [`GOAL.md`](GOAL.md); the definition of done is
> [`FINISH.md`](FINISH.md). This file is the *working* plan: what is being built right
> now, in what order, and why. Living status lives in [`docs/STATUS.md`](docs/STATUS.md);
> architectural decisions live in `docs/decisions/`; corpus fidelity notes live in
> [`docs/known-diffs.md`](docs/known-diffs.md).

## Shape of the work

The product is one vertical spine plus breadth:

```
spine:  open → parse (lossless) → render → select → edit → save → reopen elsewhere
```

Everything else (annotations, forms, pages, protect, export) hangs off the spine.
The spine is where the architecture is decided, so it is built first and deeply.

## Key architectural bets (see `docs/decisions/`)

| # | Bet | Why |
|---|---|---|
| ADR-0001 | Lossless COS layer: every object kept verbatim (raw stream bytes, original `/Filter` chain), lazily resolved from file bytes via xref; edits live in an object-level copy-on-write overlay | Enables byte-for-byte preservation of everything we do not touch (Charter §4.1 law 1) and incremental saves (FINISH S1.17) |
| ADR-0002 | Content streams are parsed into a token/operator stream that **retains byte spans**; the page-object model maps every object back to its provenance ranges | Makes surgical write-back, the Inspector, and "what changed" diffing possible |
| ADR-0003 | In-house Flate **inflate** (must return partial output for truncated streams) and in-house deflate/zlib | Standard crates abort on corruption; a truncated stream must return partial data with a diagnostic note |
| ADR-0004 | egui + eframe (winit) for the GUI, everything custom-painted from design tokens | Pure Rust, no toolkit look to fight, custom canvas, HiDPI, IME, and accessibility |
| ADR-0005 | `ttf-parser` + `rustybuzz` for shaping only; all PDF font program handling (CFF, Type 1, subsetting, CMaps, encodings) in-house | Shaping is not PDF-shaped; font program handling, subsetting, and surgical text retyping are (Charter §2.3, §4.4) |
| ADR-0006 | Permitted codecs only for JPEG (DCT) decode/encode; PNG is in-house on top of our own Flate | PNG needs exact control of predictors/bit depths; JPEG is general-purpose |
| ADR-0007 | Own analytic-coverage scanline rasterizer shared by PDF rendering *and* SVG icon rendering | One rasterizer to get right; dogfoods the renderer (Charter §8.7) |

## Milestone map (detail in GOAL §10)

| M | Deliverable | Gate it unlocks | State |
|---|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen skeleton, themed empty window at 1536×1024, icon pipeline + gallery + lint | Gate 0 | **Done** |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector object tree | M1 exit: F01–F11 | **Mostly done** (Inspector UI pending) |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell with thumbnails/zoom/nav | M2 exit: F31 p1 composition | **Mostly done** |
| **M3** | All fonts + CMaps, colour spaces, patterns, shadings, transparency, CCITT/JBIG2/JPX, OCGs | Gate 3 render fidelity | **Substantially done** (Type 3, all 16 blend modes, and `ToUnicode` landed) |
| **M4** | Page objects + provenance, select/move/scale/delete/recolour/Arrange, undo/redo, first save→reopen-elsewhere loop | U1/U2/U3 | **Done** |
| **M5** | Text model, editing, reflow, font resolution/embedding/`ToUnicode`, Text Properties panel | Gauntlet S1 | **Done at engine level** |
| **M6** | AP generators, foreign annotations, comments, add text/image/shape/link, header/footer/watermark, flatten | Gauntlet S3–S5 | **Substantially done** |
| **M7** | Fill, calculation/format/validate, Prepare Form, FDF/XFDF | Gauntlet S6 | **Substantially done** (`mangle-edit::forms` landed) |
| **M8** | Organizer, import/merge/split, retargeting | Gauntlet S7 | **Substantially done** (`mangle-edit::organize` landed) |
| **M9** | Redaction, sanitize, encryption write, signatures | Gauntlet S8–S9 | **Substantially done** (`mangle-edit::protect` landed) |
| **M10** | Export/optimize, performance, fuzzer, crash safety | Gates 2 and 7 | **Substantially done** (`mangle-edit::export` landed) |
| **M11** | UI states, full icon set, dark theme, HiDPI, responsive | Gate 8 ≥ 90 | **Next up** |
| **M12** | Gauntlet dry-run, fixes, formal run, evidence pack, independent verification | All | Planned |

## Settled subsystems

- **Syntax, Filters & Crypto (`mangle-syntax`, `mangle-filters`, `mangle-crypto`, `mangle-doc`):**
  - Classic xref tables, xref streams (`/Index`, `/W`, `/Prev`, hybrid `/XRefStm`), object streams (`/Extends`), fallback scanner recovery, and R2–R6 decryption.
  - In-house zlib/deflate/inflate, LZW, RunLength, ASCII85/Hex, PNG/TIFF predictors, and CCITT G3-1D/2D & G4 (verified byte-for-byte against `libtiff`).
  - Lossless full writer and incremental revision writer (`save_page`, `save_decoded`), plus page tree, outlines, destinations, page labels, OCGs, attachments, and metadata.
- **Fonts & Text (`mangle-font`, `mangle-text`):**
  - Embedded TrueType (`glyf` + composite glyphs), CFF Type 2 charstrings (local/global subrs, bias, `FDArray`/`FDSelect`, all 3 charset formats + Standard Strings), Type 1 (`eexec` + charstring decryption, `hsbw`, flex), Type 3 `/CharProcs` content-stream execution (`MAX_TYPE3_DEPTH = 8`), and 12 bundled OFL Liberation faces for unembedded Standard-14 and metric-compatible alias (`Arial`, `TimesNewRoman`, `CourierNew`) substitution.
  - Standard/WinAnsi/MacRoman/PDFDoc encodings, `/Differences`, 912-entry AGL, `/ToUnicode` CMap parsing (`beginbfchar` and `beginbfrange` contiguous + array targets), composite `/Type0` two-byte CIDs, `/W`/`/DW`, and `/CIDToGIDMap`.
  - TrueType subsetter (`subset_truetype`) and 5-object `/Type0` CID font embedding (`embed_truetype`) with `/ToUnicode` CMaps, plus the `--font-dirs` fallback resolver (`discover_font_files`, `inspect_font_file`, `find_system_font`, system-first `resolve_fallback`) threaded through render, session, appearance, and UI paths.
  - Geometric text extraction (both simple and `/Type0` composite fonts via `/ToUnicode`) and case-insensitive search (`mangle-text`) with byte offsets and UI search bar integration.
- **Content, Rendering & Surgical Editing (`mangle-content`, `mangle-render`, `mangle-edit`, `mangle-ui`):**
  - Tokeniser and interpreter with byte-span provenance, Form XObject recursion (`MAX_FORM_DEPTH = 12`), and all 4 PDF function types.
  - Analytic scanline rasterizer, per-pixel region clips (`W`/`W*`), all 16 PDF blend modes (12 separable + 4 non-separable `Hue`/`Saturation`/`Color`/`Luminosity`), Device/Cal/ICCBased/Indexed/Separation/DeviceN/Pattern colour spaces, axial/radial shadings, tiled patterns (`PaintType` 1 & 2), images with `/Decode`/`/ImageMask`/`/SMask` and pre-allocation bounds checks ([D18](docs/known-diffs.md#d18--an-smask-was-never-checked-against-the-image-it-masks-and-a-hostile-one-cost-a-gigabyte)), and annotation `/AP /N` rendering.
  - `PageModel` grouping, byte-range `surgery` with `q`/`Q` balance checks, surgical `edits` (move, scale, rotate, delete, recolour, crop, text state `Tf`/`Tc`/`Tw`/`Tz`/`TL`/`Ts`/`Tr`), `arrange` z-order movement with state re-materialisation, bounded `Arc`-snapshot `History`, and `Editor` session.
  - Text retyping across rungs 1–3 (`Change::Retype` in own simple or `/Type0` font via `/ToUnicode`, `Change::RetypeFace` in same-family document font, own-`/FontFile2` subset re-embedding with no notice, and `Change::RetypeIn` with bundled TrueType subset and status-bar notice) plus `replace_with_live_text` for Type 3 bitmap fonts, and paragraph `reflow` (`layout_paragraph`, `reflow_patches` with `Left`/`Center`/`Right`/`Justify` and overflow flag) plus shared-Form `inspect_shared_form`/`edit_shared_form` copy-on-write.
  - Annotations, `/AP /N` appearance generation (`Highlight` with `/BM /Multiply`, `Underline`, `StrikeOut`, `Squiggly`, `StickyNote` + `Popup` + `/IRT` replies, Unicode `FreeText`, `Line`/`Square`/`Circle`/`Polygon`/`PolyLine`/`Ink`/`Stamp`/`Caret`/`Link`/`Redact`), XFDF export/import, `/Artifact` headers/footers/watermarks, and pixel-equivalent idempotent `flatten_page` (`All`, `MarkupOnly`, `FormsOnly`, by index, `Hidden`/`NoView` skipping, and signed-document confirmation).

## Active & upcoming work queue

1. **Finish M5 UI remains (engine done):**
   - Paragraph/block width reflow and alignment (`Left`, `Center`, `Right`, `Justify` via `Tw` / line positioning) when inserted or deleted text changes line length (S1.9) — engine landed in `mangle-edit::reflow`.
   - Paragraph deletion cleanup ensuring `BT`/`ET` and `q`/`Q` remain balanced without shifting unrelated blocks (S1.10).
   - Shared Form XObject copy-on-write (S1.15): engine, refusal-by-name, headless scoped jobs, and the shell prompt dialog (`JobResult::SharePrompt` with the echoed request; single-use forms apply directly) landed; form-edit undo integration remains.
   - Custom `--font-dirs` fallback discovery for missing fonts (S1.3): flag parsing, the fallback resolver, render threading, session threading, AP embedding threading, UI plumbing, and panel family/style/colour display landed; hex-field recolour (`PanelRow::Hex` → `EditRequest::Recolour` → `Change::Recolour` in the object's own space) landed; alignment control in the Text Properties panel remains.
2. **Close open M2/M3 rendering items:**
   - Transparency groups, blend modes, and OCG visibility toggling in the renderer (`S0.3`).
   - OCG Layers panel, Attachments/Signatures panels, and Developer Inspector in `mangle-ui` (thumbnail click navigation, bookmark rows with click navigation, and the share dialog landed).
3. **M6 — Annotations, Add Content & Flatten (`mangle-edit`):**
   - Annotation data model, creation, editing, and pure deterministic `/AP /N` stream generators for all P0 types (Highlight/Underline/StrikeOut/Squiggly, Text/Note + Popup, FreeText, Line/Arrow, Square, Circle, Polygon/PolyLine/Cloud, Ink, Stamp, Caret, Link, FileAttachment, Redact).
   - Foreign annotation preservation (untouched annotations remain byte-identical on save).
   - Add content tools (Text, Image, Shape, Link in both annotation and page-content modes; Header/Footer, Watermark OCG, Background, Bates numbering).
   - Flatten command (all, by type/selection, single annotation, forms only) converting `/AP /N` into Form XObjects placed on the page and cleaning up `/Annots`.
   - FDF/XFDF import and export.
4. **M7 — Forms:**
   - Interactive AcroForm field filling, `AF*` calculation/format/validation functions, Prepare Form field creation/editing, XFA hybrid detection, and form flattening.
5. **M8 — Organize Pages:**
   - Reorder, insert (blank / from file), delete, rotate (`/Rotate`), crop (`/CropBox`), extract, split, merge, replace, cross-document resource deduplication, and bookmark/destination retargeting.
6. **M9 — Protect:**
   - Redaction mark & irreversible apply (glyph removal, image painting, metadata/layer scrub, full rewrite, leak scan), Sanitize command + report, encryption writing (RC4/AES-128/AES-256 R6), and PKCS#7/CMS SHA-256 RSA/ECDSA signing & verification.
7. **M10–M12 — Export, Optimize, Complete Fixtures, UI Polish & Gauntlet:**
   - Page raster export (PNG/JPEG at DPI), image extraction, PDF creation from images, Optimize/GC/compress, remaining Tier-A fixtures (incl. F31 `TravelGuide.pdf`), `--automation-stdio` (flag parsed with an honest unimplemented exit; the JSON-RPC line protocol is still open), and full `cargo xtask gauntlet` execution.
