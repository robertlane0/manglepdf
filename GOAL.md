# ManglePDF — Project Charter
**Prompt 1 of 2 · The Goal**

> Companion document: `FINISH.md` ("The Gauntlet") defines the only acceptable meaning of *done*. Read both documents completely before writing any code, and re-read the relevant sections at the start of every milestone.

**Conventions.** **MUST / MUST NOT** = non-negotiable. **SHOULD** = strong default; deviate only with a written reason (an ADR in `docs/decisions/`). Priority tags: **[P0]** required for v1.0 and gated by the Gauntlet · **[P1]** should ship (scored as SHOULD) · **[P2]** later. The reference screenshot of the target UI is supplied with this prompt; on day one save a copy at `docs/design/reference-mockup.png` so you can look at it again at any time with your vision.

---

## 1. Mission

Build **ManglePDF** (codename taken from the mockup; the final name may change): a desktop PDF viewer/editor written in Rust that covers the core of Adobe Acrobat Pro DC's View, Edit, Comment, Organize, Forms and Protect workflows, and does two things exceptionally well:

1. **Opens and renders very complex, sloppy, or damaged real-world PDFs** faithfully and fast.
2. **Edits *existing* page content** (text, images, vector shapes, stacking order) surgically and reliably; **adds new content** and saves it as standards-compliant **annotations** with proper appearance streams; and **flattens** annotations (all, by type, or one at a time) into page content on demand.

Everything PDF-related — syntax, object model, filters, fonts, rendering, editing, writing, encryption, signing — is written by us, in safe Rust, with no third-party PDF code.

**Trade-off order** when goals collide: (1) never corrupt or silently lose user data → (2) correctness of edits to existing content → (3) robustness on hostile/complex input → (4) rendering fidelity → (5) UI fidelity and polish → (6) breadth of features → (7) speed (subject to the hard floors in the Gauntlet).

**What success feels like:** open a gnarly file, click a headline, retype it, change its size and colour from the side panel, save — and the file opens perfectly in every other viewer, with nothing else on the page moved by a single pixel and every byte you didn't touch still identical.

You are not "done" when the feature list looks complete. You are done when the Gauntlet passes with evidence.

---

## 2. Non-negotiable constraints

### 2.1 Toolchain
- Rust **edition 2024** everywhere (`[workspace.package] edition = "2024"`, `resolver = "3"`), latest stable toolchain pinned in `rust-toolchain.toml`, `rust-version` set. One Cargo workspace.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` MUST pass on every commit to the main branch.

### 2.2 Safe Rust only (first-party code)
- `#![forbid(unsafe_code)]` at the root of **every** first-party target (libs, bins, tests, benches, examples, `build.rs`, `xtask`, tools) **and** `[workspace.lints.rust] unsafe_code = "forbid"` inherited by every member via `[lints] workspace = true`.
- No `unsafe` blocks/fns/impls/traits, no `extern` blocks, no first-party FFI, no `#[allow(unsafe_code)]`, no macros that expand to unsafe. If something seems to need `unsafe`, redesign (indices/arenas instead of raw pointers, `Arc` + channels instead of shared mutable state, safe slice APIs).
- Interpretation for third-party crates: their internal `unsafe` is outside our code and acceptable, but every dependency is a liability. Prefer few, mainstream, maintained crates, ideally ones that themselves forbid unsafe. Record each in `docs/DEPENDENCIES.md` (purpose, why not in-house, licence, unsafe status, alternatives considered). If a dependency is replaceable in < 300 lines, write it yourself.

### 2.3 No external PDF dependencies
- MUST NOT depend on, link, vendor, bind to, or (from product code) shell out to any library or tool whose purpose includes parsing, rendering, writing, editing, encrypting, signing, or extracting text from PDF. Examples (non-exhaustive): `lopdf`, `pdf`/pdf-rs, `pdf-writer`, `printpdf`, `genpdf`, `krilla`, `pdf-extract`, `hayro*`, `pdfium(-render)`, `mupdf(-sys)`, poppler bindings, cairo/Skia PDF backends, Ghostscript, qpdf, PDF.js, PDFBox/iText, pdfcpu, pikepdf.
- **Provenance test:** a crate that exists primarily to serve a PDF library/renderer counts as a PDF dependency even if it is nominally "just a codec" or "just a font tool".
- **Permitted (general-purpose, non-PDF):** GUI/windowing/GPU/file-dialog/clipboard/accessibility crates; 2D rasterization; font-file parsing, text shaping, Unicode/bidi/segmentation; general image codecs (PNG/JPEG/TIFF) and deflate; RustCrypto primitives (AES, SHA-1/2, MD5, RSA, ECDSA, DER/ASN.1, X.509, CMS/PKCS#7/#12); serde/regex/rayon/etc.; test tooling (proptest, insta, image diffing).
- **In-house (MUST be ours):** everything PDF-shaped — lexer/parser, xref + repair, object streams, writer; PDF-specific filters and codecs (LZW, RunLength, ASCII85/Hex, predictors, CCITT G3/G4, JBIG2, JPX unless a clearly non-PDF-provenance pure-Rust crate passes the fixture suite); Flate must tolerate truncated/corrupt streams and return partial output (use a crate only if it demonstrably does); Type 1/CFF/CID handling, CMaps, encodings, glyph-name tables; content-stream interpreter; page-object model and editing; annotations, forms, appearance generation; redaction; flattening; encryption handlers; signature assembly (ByteRange/CMS); optimizer.
- External tools (qpdf, mutool, poppler, Ghostscript, pikepdf, browsers, other viewers) MAY be used as **independent test oracles in dev/test scripts only** — never linked, never invoked by product code, never required for `cargo test` to pass (skip gracefully when absent).
- No C/C++ libraries for application-domain logic. The only acceptable native code is what mainstream windowing/GPU/dialog/clipboard crates bring along.

### 2.4 Platforms and offline
Windows, macOS, Linux. Develop on the host OS you are given; keep everything portable (paths, line endings, font directories, shortcuts: ⌘ on macOS, Ctrl elsewhere). Visual target: the modern macOS-like light theme in the mockup. The build MUST work offline after the first dependency fetch (document `cargo vendor` in `AGENTS.md`).

### 2.5 Assets and licences
Bundle only OFL/Apache/MIT/BSD/CC0 assets, each listed in `THIRD_PARTY_LICENSES.md`. Expect to bundle: a UI font (Inter), metric-compatible substitutes for the standard 14 fonts (e.g. Liberation), a symbol/dingbat solution, Playfair Display (the mockup's font; used in tests), a fallback strategy for CJK/Arabic/Hebrew/symbols (system fonts + optional bundled subset), Adobe's open CMap resources if licence-compatible. No Adobe branding, icons, or proprietary data.

### 2.6 Hostile-input discipline
- Library crates MUST be panic-free on untrusted input: deny `clippy::unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`; warn on `indexing_slicing` (allow only with a justification comment). Typed errors (`thiserror`) with context; graceful degradation beats failure (render what you can, mark what you can't).
- Every recursion, loop, and allocation driven by file data is bounded: depth limits, size caps, decompression-bomb guards (ratio + absolute), checked/saturating arithmetic on untrusted numbers, cycle detection everywhere (page tree, `/Parent`, `/Prev`, XObjects, patterns, Type 3, outlines, name trees, `/IRT` chains, resource inheritance).
- Long operations take a cancellation token and a time budget. The UI thread never parses, decodes, or rasterizes.
- A `catch_unwind` boundary around per-page rendering and per-command execution is a *safety net* (show an error tile, keep the app alive); a caught panic is still a bug.

---

## 3. Product scope (Acrobat Pro DC parity map)

**3.1 View & navigate** — [P0] single/continuous/two-up layouts; zoom (fit width/page, %, pinch/Ctrl-scroll, marquee); rotate view; page navigation and history; thumbnails; bookmarks panel; layers (OCG) panel with toggling; attachments and signatures panels; full-text search with highlights (case, whole-word, ligature- and diacritic-insensitive); text selection and copy; link following (GoTo/GoToR/URI/Named); full screen; Document Properties with Fonts list. [P1] cover-aware two-page spreads, split view, snapshot tool, read-only tags panel. [P2] presentation mode.

**3.2 Edit existing content** (details in §4) — [P0] text (select/edit/restyle/reflow/move/delete), images (move/scale/rotate/flip/crop/opacity/replace/export/delete), vector shapes (select/move/scale/rotate/recolour/stroke/dash/opacity/delete), Arrange (z-order), multi-select with align/distribute, copy/paste/duplicate within and between documents, inline images and Form XObjects, undo/redo. [P1] path point editing, group/ungroup, find-and-replace, vertical/RTL/complex-script editing, list/bullet awareness.

**3.3 Add content** — [P0] text, images, shapes (rectangle, ellipse, line/arrow, polygon, freehand), links (URI/GoTo/named destination) — **persisted as annotations by default** (see §5.1) with an option to save as page content; headers/footers, page numbers, watermarks (removable, layer-based), backgrounds, Bates numbering as page-content artifacts.

**3.4 Annotate & comment** — [P0] highlight, underline, strikeout, squiggly, replace/insert text (caret), sticky note + popup, free text (typewriter/text box/callout), line, arrow, rectangle, oval, polygon, polyline, cloud, ink/pencil, stamps (standard/custom image/dynamic), file attachment; comments list (author, date, status, replies, filter, sort); edit/move/delete any annotation including ones from other apps; FDF/XFDF import/export; flatten. [P1] measure tools, rich-text free text, eraser, comment summary export.

**3.5 Forms** — [P0] fill every field type; tab order; calculation, format and validation for the standard `AF*` functions implemented natively (no JS engine); FDF/XFDF import/export; clear/reset; field highlighting; **Prepare Form**: create/edit/duplicate/align fields of all types with the usual property sets (General, Appearance, Options, Actions subset, Format, Validate, Calculate), tab order, signature fields; flatten. [P1] auto-detect fields; limited safe script subset. [P2] general JavaScript. XFA: detect, preserve untouched, warn (no rendering).

**3.6 Organize pages** — [P0] reorder/insert (blank, from file, from clipboard)/delete/rotate/crop/extract/split/merge/replace; page labels; resource-safe cross-document import (dedupe by content hash, collision-safe names); bookmark/destination/link retargeting; organizer grid view. [P1] auto-crop white margins, split by bookmarks/size, batch.

**3.7 Protect** — [P0] **redaction** (mark area/text, search-and-redact with patterns, irreversible apply with overlay); **sanitize** (remove hidden information, with a report); open/owner passwords and permissions (RC4-40/128, AES-128, AES-256 R5/R6), set/remove security, honour permissions in the UI; **digital signatures**: create visible/invisible (PKCS#7 detached SHA-256 with RSA/ECDSA; PAdES baseline B-B), validate (integrity, chain to a user-managed trust store, post-signature modification detection), multiple signatures, certification (DocMDP). [P1] LTV, RFC 3161 timestamps, certificate encryption.

**3.8 Export & optimize** — [P0] export pages to PNG/JPEG at chosen DPI; extract images; export text; create PDF from images; optimize/compress (object + xref streams, dedupe, unused-object GC, image downsample/recompress presets, metadata strip); Save / Save As / Save a Copy; atomic saves; autosave and crash recovery. [P1] linearize; warn when edits break claimed PDF/A conformance; print via OS; compare two PDFs. [P2] OCR plug-in interface; PDF/A / PDF/UA conversion; auto-tagging.

**3.9 Document-level** — [P0] metadata (Info ⇄ XMP), bookmarks edit, named destinations, attachments add/remove, page-label editing, Document Properties dialog (description, security, fonts, initial view). Tagged PDF MUST NOT be corrupted by edits (MCIDs, `/StructParents`, `ParentTree`). [P1] tags panel, alt-text editing.

**3.10 App-level** — [P0] drag-and-drop open, recent files, preferences, shortcuts for everything, multiple documents (separate windows), no telemetry and no network access by default. [P1] in-window tabs, dark theme, localization-ready strings.

**3.11 Out of scope** — Acrobat Sign/Document Cloud or any cloud service; Office/HTML round-trip export beyond plain text; XFA rendering; 3D/RichMedia/Flash/audio/video playback (render posters, preserve objects); scanner acquisition; mobile; an OCR engine (leave a hook); a general JavaScript engine (preserve scripts, never run them).

---

## 4. Editing existing content — the central quality bar

This decides whether ManglePDF is a toy or a tool. Design for it from the first line of the parser.

### 4.1 The seven laws
1. **Locality** — an edit changes only the object(s) it targets. Every other operator stays byte-for-byte identical and every other pixel stays identical. No re-serializing whole pages "because it's easier".
2. **Fidelity** — what the user asked for is what appears: right font, size, colour (in its *original* colour space unless changed), position (< 0.1 pt error), stacking. No drift in neighbours.
3. **Validity** — output conforms to ISO 32000: balanced `q/Q`, `BT/ET`, `BDC/EMC`; complete resources; no dangling references; opens without repair prompts elsewhere.
4. **Reversibility** — every edit is an undo step; undo restores *structurally identical* content streams.
5. **Honesty** — if something cannot be edited faithfully (Type 3 glyphs, outlined text, missing font/glyphs with no replacement, signed/locked/permission-restricted documents), say so in the UI, explain why, and offer the closest safe alternative. Never silently drop, garble, or substitute.
6. **One source of truth** — what the canvas draws is rendered from the same regenerated content that will be saved. Gesture-time overlays (dragging, typing) MUST commit to identical output.
7. **Provenance** — every selectable object knows exactly which bytes of which content stream (and which resources) produced it. This enables surgical patches and lets the Inspector show what changed.

### 4.2 Page object model
Per page (and per Form XObject when entered) build an ordered list of **page objects** from the content streams (the `/Contents` array, Form XObjects; Type 3 glyph procs are read-only):
- **Text** → *runs* (uniform font/size/colour/matrix) → *lines* (shared baseline) → *blocks* (paragraphs/columns; use geometry, spacing, alignment, indentation, and the structure tree when present). Every glyph keeps char code, Unicode, advance, position, and origin (stream, operator index, string index, byte offset).
- **Images** (XObject, inline, stencil masks, with masks/SMasks attached); **paths** (fill/stroke/clip, compound, pattern/shading fills); **shadings** (`sh`); **Form XObjects** (group nodes that know their reference count → "shared"); **marked content** (artifacts, optional-content layers, tagged MCIDs) carried as metadata.
- Each object records: exact bounds (including stroke width and glyph bounds), CTM, full graphics-state snapshot (colour spaces + values, alpha, blend, soft mask, dash…), clip stack, z-index, containing scope, and **provenance byte ranges**.
- Grouping MUST be predictable: the selectable unit is what a designer would call "one thing" (a headline, a paragraph, a photo, a rule, a filled shape) — never "one operator".

### 4.3 Write-back (surgical by default)
- Unchanged operators are preserved byte-for-byte, including whitespace, comments, unknown operators, and inline images.
- Modify in place when possible (operands/operators inside the object's byte range). When an object must be regenerated, emit it wrapped in `q … Q` re-establishing exactly the state it needs, replacing only its range.
- New page content goes into a *new* content stream (or is spliced at its z-position) wrapped in `q … Q`. Never rewrite an existing `/Contents` wholesale.
- Deleting removes the object's operators; a `q…Q`/`BT…ET`/`BDC…EMC` wrapper is removed only if it becomes empty. Verify balance after every edit.
- **Arrange** moves operator ranges and **re-materialises graphics state** at the destination (clip, CTM, colours, text state, ExtGState) so neither the moved object nor its new neighbours change appearance.
- **Shared Form XObjects:** detect; ask *edit everywhere / this page only*; "this page only" is copy-on-write of the XObject with only this reference rewired.
- Compressed streams: decode → patch → re-encode with the original filter kind. Untouched image XObjects stay **bit-identical** on save; never recompress unless the user runs Optimize.
- Object numbers: unchanged objects keep theirs; new ones are allocated after `/Size`. Only a full rewrite with Optimize may renumber.
- After write-back, re-parse the modified region and assert it equals the intended model (always in debug builds, and in the Gauntlet).

### 4.4 Text editing
- Click selects the smallest block that behaves like one heading/paragraph; double-click enters text editing with a caret; drag/shift-arrows select; double/triple-click select word/line; Esc or click-away commits.
- The **Text Properties** panel (§8.4) edits the selection or the whole block; every control maps to real operators: font/size (`Tf`), colour (`g/rg/k/sc/scn` in the original colour space), character spacing (`Tc`), word spacing (`Tw`), horizontal scale (`Tz`), leading (`TL`/`T*` or explicit positioning), baseline shift (`Ts`), render mode (`Tr`), rotation (`Tm`); alignment is layout (recompute positions/`Tw`).
- **Line Spacing** = multiple of font size (leading = value × size). **Character Spacing** = points, converted to `Tc` given `Tz` and matrix scale. Show units in tooltips.
- **Font resolution for typed characters**, in order: (1) the run's own font if its encoding and font program contain every needed glyph; (2) same family among the document's other fonts; (3) same family from bundled/system fonts (embed a subset); (4) closest metric/style match; (5) per-script fallback chain (Latin, Greek, Cyrillic, CJK, Arabic, Hebrew, symbols). Never emit `.notdef`. Whenever (3)–(5) applies, show a non-blocking notice naming the font and reason, and let the user choose.
- **Embedding:** subset with a general TrueType/CFF subsetter (crate or in-house); emit Type 0/CIDFontType2 (Identity-H) or CIDFontType0C as appropriate with `/W`, `/DW`, `/CIDToGIDMap`, a full `FontDescriptor`, subset-tag prefix, and a correct **`ToUnicode`** so copy/search/extraction of new text is exact. Respect `OS/2 fsType` embedding restrictions and warn.
- **Layout:** untouched glyphs keep the PDF's own widths (`/Widths`, `/W`); untouched *lines* keep their original operators even when neighbours change; reflow only the edited block inside its original width, honouring alignment (left/centre/right/justified via `Tw`, last line left), indents, line spacing, bullets/hanging indents. Overflow is allowed but flagged. Existing `TJ` kerning is preserved for untouched text.
- **Special cases:** rotated/skewed text (edit in local frame; selection box rotates); vertical writing, RTL and complex scripts via a shaping crate [P1]; invisible OCR text (`Tr 3`) is an editable text layer; text as clipping (`Tr 4–7`) is editable with its mode preserved; **Type 3 and outlined text are not editable as text** → UI says so and offers *Replace with live text* / *Delete*; text in patterns/soft masks is read-only with an explanation; keep `ActualText` and marked content in sync; no automatic hyphenation.
- **Structure:** keep tagged structure valid; removed content's structure elements are removed or emptied consistently; new text is tagged Span/Artifact [P1].

### 4.5 Images
Move/scale/rotate/flip via `cm`; crop as a non-destructive clip (resettable); opacity/blend via ExtGState inside `q…Q`; replace (fit/fill/keep-size) — JPEG embedded as-is with correct `/ColorSpace` and `/Decode` (Adobe CMYK inversion) and **no re-encode**, PNG → Flate + predictors + `/SMask`, 16-bit and palette handled; export (original bytes for DCT/JPX, PNG otherwise); delete with mask cleanup; show effective PPI, colour space, compression, ICC; inline images convert to XObjects on first edit. Downsampling happens only in Optimize.

### 4.6 Vector shapes
Select whole paths, compound paths, and composites (clip + shading, pattern fills). Edit fill/stroke colour, width, dash, caps/joins/miter, opacity, blend; move/scale/rotate; delete; point/segment editing [P1]. Changing the colour of a shading/pattern fill offers *replace with solid*. Preserve colour spaces (DeviceCMYK stays CMYK; Separation stays Separation with tint edit).

### 4.7 Arrange
*Bring Forward / Send Backward* move an object past the next **overlapping** object (not the next operator); *Bring to Front / Send to Back* go to the ends of the containing scope (page or entered Form XObject). Annotations keep their own order (`/Annots`) and always draw above page content.

### 4.8 Undo/redo
Command pattern over immutable document snapshots; deep history (memory-bounded); coalesced continuous gestures; selection restored with undo/redo.

---

## 5. New content as annotations, appearance streams, saving, flattening

### 5.1 Two persistence modes
Newly added elements (text boxes, images, shapes, ink, stamps, links, form fields) are **saved as annotations by default**, each with a correct **appearance stream (`/AP /N`)**, so every viewer draws them identically and they stay editable. The user can instead choose **Page content** (per object, or as a preference) for the Edit-tab Add Text / Add Image / shape tools. Headers, footers, page numbers, watermarks, backgrounds, and Bates numbers are page-content *artifacts* (as in Acrobat). Any annotation can become page content through **Flatten**.

### 5.2 Annotation types (all with generated appearances)
Text (note), FreeText (typewriter/text box/callout/rich text), Line (arrows, caption), Square, Circle, Polygon, PolyLine (incl. cloud), Ink, Highlight, Underline, Squiggly, StrikeOut, Caret, Stamp (standard/custom/image/dynamic), FileAttachment, Link, Popup, Redact (mark-only until applied), Widget (forms), Measure [P1]. Every markup annotation carries `/NM`, `/T`, `/M`, `/CreationDate`, `/Subj`, `/Contents`, `/F` (Print), `/C`, `/CA`; replies use `/IRT` + `/RT`; review states use `/State` + `/StateModel`.

### 5.3 Rules
- AP generation is pure and deterministic. Regenerate only for annotations the user edited; **never touch foreign annotations you didn't edit** (their objects stay byte-identical).
- Implement the ISO placement algorithm (transform AP `/BBox` by `/Matrix`, map to `/Rect`); handle `/Rotate`, `NoRotate`/`NoZoom`, `/UserUnit`.
- Read foreign annotations from Acrobat, Preview, Foxit, Bluebeam, Word, LaTeX, etc.: synthesize missing APs; render odd matrices/BBoxes correctly; accept `QuadPoints` in spec order and in the Acrobat "Z" order; *write* in the order other viewers expect (TL, TR, BL, BR).
- Highlight APs use `/BM /Multiply` (plus `ca`) so text underneath stays legible.
- FreeText: text in `/Contents` (UTF-16BE when needed), `/DA`, `/DS`, `/RC`, `/Q`; the AP embeds any fonts it needs so Unicode works; AP line-wrapping matches the editor's.
- Comments panel with author, date, page, status, replies; filter/sort; FDF/XFDF export/import. Selecting an annotation uses the same chrome as page objects, with type-specific **Annotation Properties**; deleting removes its popup and reply chain.

### 5.4 Saving
- **Save** = incremental update (append changed objects + new xref section of the same kind as the previous one + trailer with `/Prev`); prior bytes and existing signatures stay verifiable. **Save As / Optimize / anything destructive** = full rewrite with mark-and-sweep GC, xref/object streams, deterministic ordering.
- Force a **full rewrite** (and say why) when: redactions were applied, sanitize ran, pages were deleted with resource GC, the file was structurally repaired on open, encryption changed, or the incremental size would exceed a bloat threshold. Repaired files show a banner: "This file had structural problems that were repaired in memory; saving rewrites it."
- Saves are **atomic** (temp file in the same directory → flush → rename); autosave snapshots go to a recovery directory and are offered at next launch.
- Preserve everything you don't understand: unknown keys, `/PieceInfo`, `/Metadata`, structure tree, outlines, OCGs, embedded files, JavaScript, `/Perms`, `/DSS`, `/AF`. Keep Info ⇄ XMP in sync on metadata edits.

### 5.5 Flatten
- Commands: **Flatten all**, **by type / by selection**, **single annotation**, **forms only**; options: keep links, keep form fields, include hidden/print-only, page range.
- Algorithm: for each eligible annotation with `/AP /N` (respect `/AS`), compute the placement matrix, register the AP as a Form XObject, append `q <matrix> cm /FmN Do Q` to the page in `/Annots` order with its blend/alpha intact; then remove the annotation (and its popup/reply chain when flattened), prune orphans, fix `/StructParents`, drop an empty `/AcroForm`. Skip `Hidden`/`NoView` annotations and report the count.
- Flatten MUST be **pixel-equivalent** to displaying the annotations, **idempotent**, and **undoable**. On signed documents, warn that it invalidates signatures and require confirmation.

---

## 6. Complex-PDF robustness catalogue (all of this MUST be handled)

**File structure** — classic xref tables; xref streams (`/Index`, varied `/W`); object streams (incl. `/Extends`); hybrid-reference files (`/XRefStm`); incremental updates (`/Prev` chains, freed/replaced objects); linearized files (hint tables may be ignored but read safely); wrong/missing xref offsets; missing or wrong `startxref`; truncated files; garbage before `%PDF-` (≥ 1 KB) and after `%%EOF`; wrong `/Length`; missing `endobj`/`endstream`; CR-only line endings; off-by-one subsection starts (`1 N` vs `0 N`); duplicate objects; object numbers beyond `/Size`; nested object-stream references; missing/wrong `/Root` (search for the catalog); page trees with cycles, wrong `/Count`, missing `/Type`, non-page kids, inherited attributes; 100k–1M objects.

**Content streams** — `q/Q` and `BT/ET` imbalance; unknown operators; `BX/EX`; inline images with abbreviations and hostile `EI` sequences; content split across arrays (including mid-token splits by broken producers); ≥ 100 MB streams; deep Form XObject nesting; Type 3 glyph procs (`d0`/`d1`); coloured and uncoloured tiling patterns; shading types 1–7 with function types 0/2/3/4 (PostScript calculator); isolated/knockout transparency groups; all 16 blend modes; alpha and luminosity soft masks (`/BC`, `/TR`); overprint (`OP/op/OPM`) at least simulated; zero-width lines; huge, tiny, negative CTM scales; degenerate paths.

**Colour** — DeviceGray/RGB/CMYK, CalGray/CalRGB/Lab, ICCBased (N = 1/3/4; v2 and v4), Indexed, Separation (incl. `/All`, `/None`), DeviceN/NChannel, Pattern, `DefaultGray/RGB/CMYK`, rendering intents, and a CMYK→RGB conversion that looks like Acrobat's rather than naive `1−c−k`.

**Images** — 1/2/4/8/16 bpc; `/Decode`; stencil masks; colour-key masks; explicit masks; `/SMask` + `/Matte`; `/SMaskInData`; Flate (PNG predictors 10–15, TIFF predictor 2), LZW (`EarlyChange` 0/1), RunLength, ASCII85, ASCIIHex, CCITT G3-1D/G3-2D/G4 (`BlackIs1`, `EncodedByteAlign`, damaged rows), JBIG2 (generic incl. MMR, symbol dictionary + text region, halftone, refinement, `JBIG2Globals`), DCT (baseline, progressive, Adobe APP14 transforms, CMYK/YCCK inversion), JPX (with/without PDF colour space, palette, alpha, 16-bit, tiles); filter chains; huge images (e.g. 20000×20000 1-bit) rendered without OOM via tiled/streamed decode; good downscaling quality.

**Fonts** — Standard 14 (with/without descriptors); TrueType (symbolic/non-symbolic, cmap (3,0)/(1,0)/(3,1)/(3,10), composite glyphs, `post` names); Type 1 (PFA/PFB, eexec, `seac`, flex); Type1C/CFF (name- and CID-keyed, subrs, hintmask); OpenType-CFF; Type 3; Type 0 with Identity-H/V; embedded and predefined CMaps (`usecmap`, variable-length code spaces); `W/W2/DW/DW2`; vertical writing; `ToUnicode` (bfchar/bfrange incl. arrays); non-embedded fonts (substitution by descriptor flags/name/metrics, `/MissingWidth`); subset-tag names and style suffixes; encodings (Standard/MacRoman/WinAnsi/MacExpert + `/Differences`; AGL, `uniXXXX`, `gNN`); text state (`Tc Tw Tz TL Ts Tr 0–7`); `TJ` arithmetic; `'` and `"`; invisible OCR text; ligatures and combining marks in extraction.

**Interactive & document features** — annotations with/without APs; every link action type; outlines with every destination type; named destinations (name tree *and* legacy `/Dests`); page labels; OCGs/OCMDs (`/VE`, `/AS`, radio-button groups); article threads; embedded files/portfolios; XMP; `/Thumb`; viewer preferences; tagged structure trees (role maps, alt text, MCIDs); AcroForms (hierarchical names, inheritance, calculation order, `NeedAppearances`); XFA-hybrid detection; existing signatures and post-signature revisions.

**Security** — RC4-40 (R2), RC4-128 (R3), AES-128 (R4), AES-256 (R5 and R6); `/Identity` crypt filters; `/EncryptMetadata false`; empty user password; owner-only protection; permission bits; encrypted object streams; Unicode/SASLprep passwords for R6.

**Scale & abuse** — 2,000+ pages; ≥ 3 M path segments on one page; ≥ 400 MB of image data; 200-deep nesting; self-referencing XObjects/patterns/Type 3 fonts/outlines; decompression bombs; malformed numbers (huge exponents, `-.`, `+-1`); NaN-producing math; mutated/fuzzed inputs. The app must degrade, never die.

---

## 7. Architecture

```
manglepdf/
  Cargo.toml                 # workspace; edition 2024; forbid(unsafe_code); shared lints
  crates/
    mangle-syntax/           # bytes, lexer, parser, objects, xref (+repair), object streams, writer primitives
    mangle-filters/          # inflate/deflate*, LZW, RunLength, A85/AHx, predictors, CCITT, JBIG2, DCT glue, JPX
    mangle-crypto/           # RC4/AES security handlers, key derivation, permissions; CMS/PKCS#7 assembly + verification
    mangle-doc/              # catalog, page tree + inheritance, name/number trees, outlines, dests, labels, OCG, files, XMP/Info
    mangle-font/             # TrueType/OpenType/CFF/Type1 programs, CMaps, encodings, AGL, metrics, subsetting, substitution
    mangle-content/          # content-stream parse/serialize, graphics state, page-object model with provenance
    mangle-render/           # rasterizer, colour, images, shadings, patterns, transparency, glyph cache, tiles, display lists
    mangle-text/             # extraction, layout analysis (runs/lines/blocks), search, typesetting/reflow
    mangle-edit/             # commands + undo, object edits, annotations + AP generation, forms, redaction, flatten, page ops, optimize
    mangle-ui/               # GUI app: theme tokens, icons, widgets, canvas, panels
    mangle-cli/              # headless CLI + automation used by tests and the harness
    mangle-testkit/          # image diff (max/RMS/SSIM), invariants, fixture manifest, evidence writer
  tools/fixturegen/          # INDEPENDENT fixture generator (must not depend on any product crate)
  xtask/                     # policy, fixtures, gauntlet, fuzz, perf, icons
  assets/{icons,fonts,cmaps,theme}   docs/   fixtures/   corpus/ (gitignored)
```

Crate boundaries may change with an ADR; **dependency direction may not invert** (syntax → filters/crypto → doc → font → content → render/text → edit → ui/cli).

Principles:
- **Lossless COS layer.** An object graph that retains every object verbatim (raw stream bytes + original filter chain), lazily loaded from the file bytes by xref offset. Modified objects live in an overlay (object-level copy-on-write). Incremental save writes the overlay; full save writes the reachable graph.
- **Immutable snapshots + command log.** `Arc` snapshots make undo/redo cheap and let background rendering work on a consistent view while the user keeps editing.
- **Provenance-aware parsing.** The content parser yields operators with byte spans; the object model maps back to those spans.
- **Rendering pipeline.** Per-page display list cached by content generation → tile renderer with LRU caches → progressive refinement (fast low-res first, sharp on idle) → cancellation on scroll/zoom. Deterministic output regardless of thread count.
- **Determinism.** No `HashMap` iteration in any output path; injectable clock/ID/random; `SOURCE_DATE_EPOCH` respected. Same input + same script ⇒ byte-identical output.
- **Numbers.** Geometry in `f64`. PDF numbers written without exponents, ≤ 6 decimals, never NaN/inf.
- **Concurrency.** Worker pool for parse/decode/raster; the UI thread only composes and handles input; channels + cancellation tokens; shared state only behind explicit `Arc<RwLock<…>>` boundaries.
- **Memory.** LRU budgets for decoded streams, glyphs, tiles; big images decoded in strips/tiles.
- **Errors.** `Result` end-to-end; each error has a user-facing message and technical detail (visible in the Inspector).
- **Hooks for later:** OCR provider trait, script-engine trait.

---

## 8. UI specification (derived from the mockup)

### 8.1 Feel
Calm, modern, light, macOS-adjacent: generous whitespace, hairline dividers, soft 8 px radii, a single blue accent, restrained shadows, large legible labels, everything on a 4 px grid. The document is the hero; the chrome is quiet. The screenshot is a **feel-and-layout reference, not a pixel spec** — it is an AI-generated mockup, so where it is ambiguous, inconsistent, or warped (icons, thumbnail micro-text, the photo), follow Acrobat/macOS conventions and good taste.

### 8.2 Layout (reference window 1536×1024 logical; content ≈ 1440×936)
Top to bottom:
- **Title bar (~52 px).** Window controls at far left (native traffic lights on macOS; native chrome elsewhere — do not fake them). Brand label "ManglePDF" (semibold ≈ 16 px) beside them. **Centred document title** "TravelGuide.pdf" with a small chevron opening a document menu (rename, show in folder, properties, recent). At right: a **search field** (magnifier, placeholder "Search (⌘F)", ≈ 275 px wide, 8 px radius, hairline border) and a **Share** icon button (menu: save a copy, export pages as images, print).
- **Tool-tab row (~72 px).** Seven items, icon above label, evenly spaced and centred: **View · Edit · Annotate · Page · Form · Protect · More (⋯)**. The active tab is a pale-blue rounded rectangle (≈ 60×58 px) with blue icon and label (in the mockup: Edit). Hairline bottom border.
- **Body, three columns:**
  - **Left sidebar (~15% width, ≈ 222 px).** Two small toggle icons at top (thumbnail grid = pages; list = bookmarks/outline; extendable to layers, attachments, signatures, comments, search results). Below: a scrolling column of **page thumbnails** with the page number centred underneath each. The current page has a 2 px blue rounded border and a pale-blue number pill; others have soft shadows. Thin overlay scrollbar.
  - **Canvas (~58%).** Neutral light-grey backdrop; page centred with a soft shadow. **Selection chrome:** 1 px blue outline with small white square handles (blue border) at the corners and edge midpoints; hover shows a thin blue outline.
  - **Right panel (~26%, ≈ 381 px).** White, contextual (§8.4), collapsible sections with chevrons separated by hairlines, 24 px padding.
- **Bottom bar (~50 px)** under the canvas: at left zoom **− · 100% · +** (the percentage opens presets); centred **‹ [ 1 ] / 12 ›** with an editable page box; at right a **fit/full-screen** button.

### 8.3 Design tokens (approximate — sample real pixels from the mockup's *chrome* with a small script and record final values in `assets/theme/tokens.toml`)
- Accent blue ≈ `#3B6CF0`; accent tint (active tab/segment) ≈ `#E4ECFD`; selection outline = accent.
- Text primary ≈ `#1F2430`; secondary ≈ `#6B7280`; disabled ≈ `#A3A9B5`.
- Hairline ≈ `#E4E7EC`; input border ≈ `#D5D9E0`; canvas backdrop ≈ `#EEF0F3`; sidebar ≈ `#F6F7F9`; panel/title ≈ `#FFFFFF`; tab row ≈ `#FBFCFD`; mockup's navy swatch ≈ `#0F1F4B`.
- Type: Inter (or system UI). Title 15/600 · tab label 12.5/500 · section header 15/600 · field label 13.5/400 secondary · value 14/500 · monospace for the Inspector.
- Radii: 8 px (inputs, segmented controls, buttons), 12 px (popovers). Borders 1 px. Shadows: page `0 2 12 rgba(0,0,0,.18)`, popover `0 8 24 rgba(0,0,0,.16)`.
- Spacing on a 4 px grid; control height 36–40 px; tab icons 22–24 px; panel icon buttons 20–24 px.
- Motion: 120–160 ms ease-out for hover/press/popovers; none under reduce-motion.

### 8.4 Contextual right panel
**Text selected ("Text Properties", exactly as in the mockup):** (1) content field showing the selected text (multi-line for blocks); (2) font-family dropdown ("Playfair Display") + style/weight dropdown ("Bold"); (3) size stepper "− 48 pt +"; (4) circular colour swatch opening a popover (swatches, document colours, hex, eyedropper, opacity); (5) four-segment alignment control (left/centre/right/justify; left active); (6) "Line Spacing" stepper (1.4); (7) "Character Spacing" stepper (0). Then **Arrange** (Bring Forward · Send Backward · Bring to Front · Send to Back — icon over label, four across) and **Actions** (Rotate · Delete · Replace — three across). Advanced options (word spacing, horizontal scale, baseline shift, render mode, stroke, opacity, underline/strike) live under a "More options" disclosure. Every selection type also gets a **Position & Size** section (X, Y, W, H, rotation).

**Other selections:** *Image* → pixel size, effective PPI, colour space/compression/ICC readouts, opacity, blend, flip H/V, crop, export, replace. *Shape* → fill (colour/none/gradient-aware), stroke (colour, width, dash, cap, join), opacity, blend. *Annotation* → type-specific colour/opacity/thickness/style/line endings, author, subject, contents, status, lock, **Flatten this annotation**. *Form field* → tabbed properties. *Link* → action type, destination, appearance. *Multiple objects* → align/distribute/group with "Mixed" values shown. *Nothing selected (Edit tab)* → tool cards (Add Text, Add Image, Link, Header & Footer, Watermark, Background, Bates) and the hint "Click any text or image on the page to edit it."

### 8.5 What each tab contains
- **View** — layout, zoom, rotate view, select/hand/marquee-zoom; right panel accordion: Document info, Bookmarks, Layers, Attachments, Signatures, Tags.
- **Edit** — contextual editing (above).
- **Annotate** — tool strip (highlight, underline, strikeout, squiggly, note, text box, callout, pencil/eraser, line/arrow/rectangle/oval/polygon/polyline/cloud, stamp, attach, measure); right panel shows tool/selection properties + **Comments** list.
- **Page** — organizer grid on the canvas; right panel: insert, delete, extract, split, rotate, crop, replace, labels, header/footer, watermark, background, Bates.
- **Form** — Fill ⇄ Prepare; field tool palette, field list, properties, tab order.
- **Protect** — encrypt/permissions; redaction (mark area/text, search & redact, apply); sanitize; sign/validate.
- **More** — export, combine, optimize/compress, compare [P1], document properties, preferences, Developer → Inspector.

### 8.6 Interaction requirements
- Click selects the object under the pointer; double-click on text edits; click empty space deselects; Shift-click/marquee multi-select; arrow keys nudge 1 pt (Shift = 10 pt); handles resize (Shift = proportional, Alt = from centre); a rotation handle for rotatable objects; smart guides [P1].
- Zoom via pinch/Ctrl-scroll centred on the pointer; pan via scroll or Space-drag; progressive rendering (instant low-res, then sharp); skeleton placeholders for unrendered pages; the UI stays responsive (frame time ≤ 33 ms) during any load or render.
- Panel controls apply live; numeric fields accept typing, arrow keys, Shift ×10; stepper holds coalesce into one undo step; "Mixed" shown for differing values.
- Cursors match tools (text, move, resize, rotate, hand, crosshair); tooltips show shortcuts; context menus everywhere.
- Notices are non-blocking banners (font substitution, repaired file, permission limits); modal dialogs only for destructive confirmations and passwords.
- Keyboard-navigable with visible focus rings; expose an accessibility tree if the toolkit supports it [P1].

### 8.7 Icons — hand-authored SVG, never cropped
- Author **every** icon by hand as SVG (you write the coordinates). You MAY study the mockup for metaphor and style; you MUST NOT crop, trace, auto-vectorize, or embed pixels from it (some mockup icons are AI-warped — do not inherit that).
- Style: 24×24 viewBox, 2 px safe padding, 1.75 px stroke, round caps/joins, `fill="none"`, `stroke="currentColor"` (tinted from tokens), 2 px corner radii, consistent metaphors, no text, no gradients, elements limited to `path/line/polyline/polygon/rect/circle/ellipse/g`, < 2 KB each, one file per icon at `assets/icons/<name>.svg`. An optional filled "active" variant is fine.
- Runtime: render through a small in-house SVG-subset renderer built on the same vector rasterizer as the PDF renderer (dogfooding), or a general-purpose SVG crate if justified in an ADR. Cache by (name, size, scale, tint). Crisp at 1×/2×/3×.
- **P0 inventory (≈ 100):** tool tabs (view, edit, annotate, page, form, protect, more) · title bar (search, share, chevrons) · sidebar (thumbnails-grid, list/bookmarks, layers, attachments, signature, comments) · bottom bar (minus, plus, prev, next, fit/fullscreen) · text (align-left/centre/right/justify) · arrange (bring-forward, send-backward, bring-to-front, send-to-back) · actions (rotate, trash, replace-image) · image (crop, flip-h, flip-v, opacity, export, eyedropper) · shapes (rectangle, ellipse, line, arrow, polygon, polyline, cloud, pencil, eraser) · markup (highlight, underline, strikethrough, squiggly, note, text-box, callout, stamp, paperclip, measure, caret-insert, caret-replace) · pages (insert, delete, extract, split, merge, crop-page, rotate-left/right, reorder, header-footer, watermark, background, bates) · forms (text-field, checkbox, radio, dropdown, list, button, signature-field, date, tab-order) · protect (lock, unlock, key, redact, sanitize, certificate, shield-check) · generic (open, save, save-as, undo, redo, copy, paste, duplicate, eye, eye-off, link, unlink, bookmark, close, check, warning, info, error, settings, search-next/prev, hand, cursor, marquee-zoom, zoom-in/out, align-*, distribute-*, group, ungroup, folder, file, download, upload).
- **Gallery + lint.** `cargo xtask icons` renders every icon at 16/20/24/32 px × 1×/2× × light/dark onto one sheet that you review via screenshot whenever icons change. The lint checks viewBox, allowed elements, stroke-width uniformity, no hard-coded colours, size cap, safe-area bounds, and no `<image>`/base64.
- Toolbar/panel icons exist by M2; the full P0 set by M11.

### 8.8 UI engineering
- Toolkit is your choice (e.g. egui/eframe, iced, slint). Requirements: custom painting for the canvas, HiDPI, IME text input, drag-and-drop, native file dialogs, clipboard, shortcuts, window-state persistence. **Theme everything from tokens** — the toolkit's default look is unacceptable. Keep all widgets in `mangle-ui` so restyling is central.
- Default window 1536×1024 logical, minimum 960×640. Below ≈ 1100 px width the sidebar collapses to icons and the right panel becomes a collapsible overlay.
- CLI flags: `--window-size WxH --scale N --theme light|dark --reset-prefs --open FILE --automation-stdio --font-dirs PATH[,PATH]` (`--font-dirs` replaces system font discovery so font-fallback behaviour is testable and reproducible).

---

## 9. Verification strategy

1. **Layers of tests.** Unit; property tests (parse∘write round-trips, transform algebra); golden images with tolerance; fixture-manifest integration tests; safe mutation fuzzing; differential tests against oracles; UI automation; computer-use runs.
2. **Fixtures.** `tools/fixturegen` builds deterministic torture PDFs from scratch with its own tiny writer and **no dependency on product crates** (independence matters: a generator that shares the parser's misunderstandings proves nothing). A manifest records features, expected page counts/text/object counts, and checksums. Real-world PDFs go in `corpus/wild/` (gitignored, with `SOURCES.md` and licences) — download from public corpora if the network allows (PDF.js, qpdf and poppler test files, veraPDF corpus, Ghent output suite, GovDocs1), or ask the human. `corpus/heldout/` is reserved for the human.
3. **Oracles.** If present on the machine, use qpdf/mutool/pdftoppm/pdftotext/pdfinfo/Ghostscript/pikepdf and real viewers (Chrome/Edge PDFium, Firefox PDF.js, Preview, Evince/Okular, Reader) to cross-check validity and appearance. Never in product code; tests skip cleanly when absent.
4. **Golden discipline.** Look at every new baseline with your vision (and against an oracle when possible) *before* blessing it — a golden generated by the code under test is not proof. Compare with max-delta, RMS, windowed SSIM, and "pixels above AA tolerance"; store diff heatmaps.
5. **Invariants to test everywhere:** open→save (no edits)→open is semantically identical and pixel-identical; edit+undo restores content streams; edits change nothing outside the target's bounds; flatten is idempotent and pixel-equivalent; tile-stitched render equals whole-page render; results are identical across thread counts; same input + script ⇒ identical bytes.
6. **Fuzzing without `unsafe`.** Write a structure-aware and byte-level mutation fuzzer in safe Rust as an `xtask` (no libFuzzer): watchdog timeouts, memory caps, minimized repros committed to `tests/regressions/`.
7. **Driving the UI.**
   - `--automation-stdio` speaks JSON-RPC lines: `open, save, screenshot, click, drag, key, type, scroll, get_ui_tree, get_selection, get_object_model, get_state, wait_idle, quit`. Synthetic input MUST flow through the same input pipeline as real input (no back doors into internals).
   - A **developer Inspector** (More → Developer): object tree, content-stream disassembly with operator↔object highlighting, provenance of the object under the pointer, and a content-stream diff after each edit. Debug overlays: bounds, z-order numbers, text-run/line/block segmentation, font info.
   - **Computer-use loop every milestone:** launch → screenshot → act with real mouse/keyboard → screenshot → compare to the mockup → list ten nits like a designer → fix → repeat.
8. **Policy checks.** `cargo xtask policy` enforces edition 2024, `forbid(unsafe_code)` everywhere, no banned/PDF-provenance dependencies (scan `cargo metadata` names, descriptions, keywords, repositories), no `Command::new` in product code except OS integration, lint config, asset-licence coverage.

---

## 10. Working method

**Memory lives in the repo.** Keep `AGENTS.md` (how to build/test/run, conventions, pitfalls; < 200 lines; always accurate), `docs/STATUS.md` (living dashboard: current milestone, what works, what's broken, next five tasks, known bugs), `docs/decisions/NNNN-*.md` (ADRs), `docs/PDF-QUIRKS.md` (every weird real-world thing you learn, with its fixture), `docs/TESTING.md`. A fresh context must be able to resume from these alone.

**Discipline.** Small commits, each green on fmt/clippy/test; never leave main red; WIP behind feature flags. Vertical slices first (open → render → select → edit → save → reopen elsewhere), then breadth. Attack the hard problems early — surgical write-back and text editing shape the architecture, so design provenance into the parser in M1.

**Stuck protocol.** Time-box. After three failed approaches, write down what you tried in `STATUS.md`, take the simpler alternative or defer with a *visible* UI notice (never a silent gap), and move on.

**Honest reporting.** Never write "done" or "works" without evidence (test name, screenshot path, file hash). Distinguish *implemented → tested → verified in the GUI → verified in other viewers*.

**Ask the human only** for things only they can supply: held-out PDFs, certificates, machine access, licensing decisions. Otherwise decide, record an ADR, proceed. Don't gold-plate P2 before the Gauntlet passes.

**Milestones (each ends with a screenshot session and a `STATUS.md` update):**
- **M0 Skeleton & policy** — workspace, lints, `xtask policy`, `AGENTS.md`/`STATUS.md`, fixturegen skeleton, empty themed window at 1536×1024 with the layout regions and tokens, icon pipeline + gallery + lint. *Exit:* policy green; shell screenshot beside the mockup.
- **M1 Syntax & document core** — parser/xref/repair/object streams/decryption; page tree; full + incremental writer; round-trip tests; Inspector object tree. *Exit:* open/save/reopen fixtures F01–F11 with no semantic change.
- **M2 Renderer v1 + viewer** — interpreter, paths/clips/text (embedded TrueType/CFF, standard 14), Flate/DCT, Gray/RGB/CMYK; tiles; viewer with thumbnails/zoom/navigation. *Exit:* TravelGuide page 1 looks like the mockup composition.
- **M3 Renderer v2** — all font types and CMaps, remaining colour spaces, patterns, shadings, transparency/blend/soft masks, CCITT/JBIG2/JPX, OCGs; oracle comparisons. *Exit:* render-fidelity thresholds on F12–F22.
- **M4 Page objects + surgical write-back** — provenance model; select/move/scale/delete/recolour/Arrange for images and shapes; undo/redo. *Exit:* first edit → save → reopen-in-another-viewer loop.
- **M5 Text editing** — text model, editing, reflow, font resolution/embedding/`ToUnicode`, all Text Properties controls. *Exit:* Gauntlet stage 1 passes headless.
- **M6 Annotations, add content, flatten** — AP generators, foreign-annotation handling, comments panel, add text/image/shape/link, header/footer/watermark, flatten. *Exit:* stages 3–5.
- **M7 Forms** — fill, calculations, Prepare Form, FDF/XFDF, flatten forms. *Exit:* stage 6.
- **M8 Pages** — organizer, import/merge/split, retargeting. *Exit:* stage 7.
- **M9 Protect** — redaction, sanitize, encryption write, signatures. *Exit:* stages 8–9.
- **M10 Export, optimize, scale, robustness** — export/optimize, performance, safe fuzzer, crash safety. *Exit:* Gates 2 and 7.
- **M11 UI polish** — states, full icon set, dark theme [P1], HiDPI, responsive layout. *Exit:* UI rubric ≥ 90.
- **M12 Acceptance** — dry-run the Gauntlet, fix, run formally with evidence, hand to an independent verifier.

---

## 11. PDF field notes (hard-won facts — verify against the spec, then encode as tests)

**Syntax & structure**
- Rebuild the xref by scanning for `N G obj` whenever anything looks off; later definitions win; scan object streams too; choose the trailer that has `/Root`, else find `/Type /Catalog`.
- `/Length` lies: trust it only if `endstream` follows (allowing EOL), else scan. `stream` is followed by CRLF or LF (tolerate a lone CR).
- A `/Contents` array is one logical stream: join with whitespace; tokens shouldn't span streams, but broken producers do it anyway.
- Inline images: expand abbreviations; find `EI` via filter/length when known, otherwise whitespace + `EI` + whitespace/EOF followed by plausible operators.
- `/Resources`, `/MediaBox`, `/CropBox`, `/Rotate` inherit down the page tree; `/Count` is unreliable; `/Type` may be missing; pages may share resources and streams.

**Content semantics**
- `W`/`W*` take effect after the next painting operator (usually `n`) and last until the enclosing `Q`.
- Text advance: `tx = ((w0 − Tj/1000)·Tfs + Tc + Tw)·Th`. `Tw` applies only to single-byte code 32 (never multi-byte CID codes). `TJ` numbers are thousandths of text space and *subtract*. Vertical mode uses `W2/DW2` and position vectors.
- `Tr 3` is invisible (OCR layers); `Tr 4–7` add to the clip at `ET`.
- Type 3: `d1` glyphs are uncoloured masks (ignore colour operators inside); `d0` glyphs set their own colours; glyph procs use `FontMatrix` and their own resources (falling back to the page's).
- A pattern's matrix maps pattern space to the default coordinate space of the *parent content stream* (page or form), not the current CTM.
- Zero-width lines render as the thinnest line the device can draw; sub-pixel non-zero lines must not vanish.
- A Form XObject without `/Resources` uses the page's; honour `DefaultGray/RGB/CMYK`.
- Optional content: `/OC` on XObjects/annotations and `BDC /OC /Name … EMC`; visibility follows `/OCProperties /D` (`/BaseState`, `/ON`, `/OFF`, `/AS` usage for View/Print).
- Text extraction priority: `ToUnicode` → encoding + AGL → font `cmap`/`post` → `ActualText` → U+FFFD; decompose ligatures; infer spaces from gaps; follow the structure tree's reading order when tagged.

**Fonts**
- Simple TrueType, symbolic: cmap `(3,0)` with `code` or `0xF000+code`, else `(1,0)`. Non-symbolic with `/Encoding`: glyph name → Unicode (AGL) → `(3,1)`, else `(1,0)` via MacRoman, else `post` names.
- CID → glyph: `CIDFontType2` uses `/CIDToGIDMap` (Identity or stream); `CIDFontType0` with CID-keyed CFF maps CID through the charset, while OpenType-CFF/name-keyed data with Identity behaves as GID.
- Use `/Widths` to lay out *existing* glyphs (that's what the original renderer did); use the font program's metrics for *new* glyphs.
- Subset tags (`ABCDEF+Name`) must be unique per subset; a new subset of the same font gets a new tag.
- Standard-14 substitutes must match AFM widths so layout doesn't shift.
- Photoshop-style CMYK JPEGs are inverted: embed with `/Decode [1 0 1 0 1 0 1 0]` or handle at decode.

**Annotations & forms**
- AP placement: transform `/BBox` by `/Matrix` → axis-aligned box; scale/translate that box onto `/Rect`.
- A widget and its field may be one dictionary or parent/child; `FT/Ff/V/DV/DA/Q` inherit. Checkbox/radio APs are dictionaries keyed by state name; `/AS` selects; the `Off` state may be absent; `/Opt` maps export values; unison radios need `RadiosInUnison`.
- Text-field APs use `/Tx BMC … EMC` clipped to the inner box; font size 0 = auto-size (compute it); comb fields divide `/MaxLen` cells; multiline wraps; `/Q` justifies.
- `NeedAppearances true` asks viewers to regenerate; always write real APs and clear the flag once you have.
- Popups are separate annotations linked through `/Popup` and `/Parent`; deleting a markup deletes its popup and replies.
- Dates: `D:YYYYMMDDHHmmSSOHH'mm'`. Text strings: PDFDocEncoding or UTF-16BE with BOM (PDF 2.0 also allows UTF-8 with BOM).

**Writing, security, redaction**
- Incremental update: append changed objects + xref section (same kind as the previous) + trailer with `/Prev`, same `/Root`, updated `/Size`, same `/ID[0]`; a linearized file stops being linearized after an update (still valid).
- Full save: mark-and-sweep from trailer roots (`/Root`, `/Info`, `/Encrypt`), keep unknown-but-reachable objects, pack non-stream objects into object streams, write an xref stream, order deterministically.
- Signatures: reserve a fixed-size hex `/Contents` placeholder (8–32 KB), compute `/ByteRange [0 a b c]` over everything except that string, hash, build the CMS, patch in place; afterwards only incremental updates may follow.
- Encryption: strings and streams are encrypted per object (key from object/generation for RC4/AESV2; AESV3 uses the file key directly); xref streams and the `/Encrypt` dictionary's own strings are not encrypted; objects inside object streams are not individually encrypted (the container is); AES = CBC, random 16-byte IV prefix, PKCS#5 padding; R6 uses the iterated SHA-256/384/512 + AES hash (Algorithm 2.B); `/EncryptMetadata false` leaves XMP in the clear.
- Redaction removes data, it doesn't hide it: rewrite operators (split `TJ` strings and compensate advances), blank pixels in decoded images and re-encode, clip/remove paths, delete annotations/fields/metadata in the region, drop or regenerate `/Thumb`, then **full-rewrite** so old revisions can't leak.
- Numbers: never exponents, NaN, or inf; strip trailing zeros; ≤ 6 decimals.

---

## 12. Deliverables and references

**Deliverables:** the repo per §7; binaries `manglepdf` (GUI) and `manglepdf-cli`; `AGENTS.md`, `README.md`, `docs/{ARCHITECTURE,STATUS,DEPENDENCIES,PDF-QUIRKS,TESTING,ICONS}.md`, `THIRD_PARTY_LICENSES.md`, ADRs, `docs/design/` (mockup + tokens); `cargo xtask {policy,fixtures,gauntlet,fuzz,perf,icons}`; assets with licences; and the acceptance Evidence Pack defined in the companion prompt.

**References (cited from memory — obtain and read the real documents; never implement from memory):** ISO 32000-2 (PDF 2.0) and ISO 32000-1 (PDF 1.7) as the primary specification; Adobe's supplement covering AES-256 R5 ("Extension Level 3"); Adobe technical notes on CMap/CIDFont files, the Type 1 font format, CFF, and Type 2 charstrings; the Adobe Glyph List specification; PDF Association interoperability guidance (annotation appearances, QuadPoints, etc.); public test corpora (PDF.js test PDFs, qpdf and poppler test files, veraPDF corpus, Isartor suite, Ghent output suite, GovDocs1) if the network allows.

**When in doubt:** preserve, don't destroy; tell the user the truth in the UI; prove every claim with evidence.
