# Status

Where the work actually is. Updated whenever a milestone moves.

A word about the tests here, because it is the pattern rather than any single finding. Four
of the real defects this project has had were invisible to its own suite and were found by
comparing against something outside: a page rendered at scale squared, a clip transformed
twice, a test helper missing an absolute value, and a compression format that only this
project could read. Each was masked by the same thing — fixtures written to be simple, and
therefore symmetric. A symmetric page renders correctly even when it is rendered wrongly, so
the shape of a fixture decides how much of a renderer's arithmetic it actually exercises.

## Milestones

| M | Deliverable | State |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen, window shell, icon pipeline | **done** — every Gate 0 check passes; the window draws the six regions from tokens and nothing else |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector | **mostly done** — everything except the Inspector. See "Gaps" below |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | **partly done** — the tokeniser, the operator table, the graphics state and the interpreter exist, and the rasterizer now turns a page's paths, its embedded-TrueType glyphs and its embedded-CFF glyphs into pixels with analytic coverage. Images and shadings draw; patterns and the standard fourteen still draw nothing |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | **partly done** — the four PDF function kinds, axial and radial shadings and the device colour spaces paint, and TrueType, composite, CFF and Type 1 outlines all draw. The standard fourteen, Type 3, mesh shadings, tiling patterns and transparency do not |
| **M4** | Page objects, select/move/scale/recolour, undo/redo, first save→reopen | not started |
| **M5**–**M12** | Text, annotations, flatten, forms, organize, redact, signatures, export, UI polish, gauntlet | not started |

## What exists

- **Workspace** — one crate graph, edition 2024, `resolver = "3"`, pinned toolchain,
  shared lints, `forbid(unsafe_code)` everywhere, and `cargo clippy` clean.
- **`cargo xtask policy`** — Gate 0, and it passes. An AST scan (with `syn`, not a
  regex) proves there is no `unsafe` and no `allow(unsafe_code)`; the dependency graph
  is audited against the banned and PDF-provenance lists with every crate accounted for
  in `docs/DEPENDENCIES.md`; process spawning is confined to one audited module and to
  test oracles; the docs, the licences and the icons are checked; and fmt, clippy, the
  tests and a release build all run.
- **`tools/fixturegen`** — 12 Tier-A fixtures with a manifest, generated from the
  specification by a writer that shares no code with the product. Each records what an
  independent validator is expected to say about it, and whether our reader has to
  rebuild its structure — two different questions, both answered.
- **`mangle-filters`** — in-house inflate and deflate, LZW, RunLength, ASCII85, ASCIIHex,
  PNG and TIFF predictors, CCITT G3-1D/2D and G4. Every decoder returns partial output
  with a note rather than failing. What we write is a zlib stream, because `/FlateDecode`
  names one, and a file no other program can open is the one thing a PDF writer must not
  produce.
- **`mangle-crypto`** — RC4, AES-CBC, MD5, SHA-1, SHA-2. The standard security handler
  for revisions 2 to 6, checked against dictionaries an independent implementation
  produced.
- **`mangle-syntax`** — the object model, lexer, parser, cross-reference tables and
  streams with `/Prev` and hybrid `/XRefStm` chains, object streams, recovery by
  scanning, decryption, a copy-on-write document overlay, and **both writers**: a full
  rewrite that keeps every object number, and an incremental revision that can express
  a deletion and carries `/ID` forward.
- **`mangle-doc`** — the page tree with attribute inheritance, name trees, outlines and
  destinations, page labels, optional-content groups, attachments and metadata.
- **`mangle-font`** — font programs, encodings, metrics, and the outlines a glyph is filled
  from.
  **Metrics** are real: `/Widths` indexed from `/FirstChar`, the descriptor's
  `/MissingWidth` for a code the run does not reach, and the fourteen standard fonts built
  in. Every width in every table was checked against two independent renderers and against
  Adobe's own metrics files, which is how `fraction` in Helvetica was caught carrying the
  width of a locally-installed metrically-similar clone — 278 where Adobe says 167. All 1043
  widths now agree with the metrics files. **Outlines** come from embedded TrueType through
  a table walk: the (3,0), (1,0) and (3,1) cmaps in that order, falling back to treating the
  character code as a glyph index, which is what a subsetted symbolic font needs. Outlines
  are returned in ems and scaled by the em size, and the composite glyphs TrueType's format
  is full of come out whole.

  **Encodings resolve a code to a glyph *name*, which is what decides the glyph.**
  `StandardEncoding`, `WinAnsiEncoding`, `MacRomanEncoding` and `PDFDocEncoding` are here as
  tables from code to name, transcribed from Table D and Annex D.2 and cross-checked against
  three sources already on the machine; the AGL resolver holds 912 names from Adobe's own
  Glyph List, Greek and Cyrillic included. All four of the shapes a file writes its
  `/Encoding` in are read — a bare name, a dictionary naming a base, that dictionary with a
  `/Differences` array, and a dictionary with no base at all — and a `/Differences` run that
  goes past the end of the base fills the gap. The renderer's `Mark::Glyphs` arm asks the
  font's encoding for the code's name before it asks the program for an outline, so a page
  that remaps one code now draws that code's glyph. A composite font is left alone: its code
  is a CID, not a character code, and the specification says `/Differences` does not apply
  to one.

  **CFF outlines are executed rather than walked.** A `CFF ` table's glyphs are Type 2
  charstrings — programs, not point lists — so reading one means running it: a stack
  machine over `f32` with local and global subroutines, the subroutine-number bias, stem
  hints, and a width that may or may not be the first operand on the stack. What is covered
  is every drawing operator (the movers, the liners, all seven curve forms and the four
  flexes), the stem hints and both mask operators with the mask length the stems imply,
  `callsubr`/`callgsubr`/`return`, `endchar`, and the CFF 2 `vsindex` and `blend`. Bounds
  are the format's own — 48 values on the stack and 10 nested calls — and passing either is
  reported with a reason rather than accommodated.

  Three things are **not** covered, and each says so rather than drawing something plausible:

  - **A bare `/Subtype /Type1C` font with a name-keyed charset cannot resolve a character
    code.** The codes in a content stream name glyphs through the font's `/Encoding`, and the
    charset that turns those names into glyph numbers is a table this does not read. The
    *outlines* are read from that same table, so the report says exactly that rather than
    leaving a page of blank where the text should be. The `/Subtype /OpenType` form of the
    same font is fully supported, because its `cmap` beside the `CFF ` table answers the
    question.
  - **A CID-keyed CFF is read only through the identity charset.** `/FDArray` and
    `/FDSelect` are both read, so each glyph gets its own Private DICT, its own subroutines
    and its own widths — which is the part that is easy to get wrong and silent. What is not
    read is a charset that renumbers, so such a font is refused with a reason: treating its
    identifiers as glyph numbers would draw the wrong letter for every character and nothing
    downstream could tell.
  - **`seac` is refused**, and so is a font whose Top DICT says its charstrings are Type 1
    (`CharstringType 1`). Both build an accented character out of two others *by name*, and
    drawing the unaccented one instead would be a wrong shape that looks like a right one.

  What *is* left out on purpose: every hinting value except the stem count (`BlueValues`,
  `StdHW`, `ExpansionFactor` and the rest describe how to snap stems to a rasterizer's pixel
  grid, and this renderer computes exact analytic coverage and has no grid to snap to), the
  variation store's region scalars (so a `blend` is its default value, which is a CFF2
  font's default instance and is what a PDF asks for), and the escaped transient arithmetic
  operators, which no real font uses.
- **`mangle-content`** — content streams. Every token, every operator and every mark
  carries the bytes it came from, which is what makes a selection a byte range and an
  edit a single rewrite. The operator table is the specification's, the graphics state is
  a value rather than a place, and a test proves across the whole corpus that every mark
  names bytes that are inside the page it came from.
- **`mangle-render`** — the rasterizer. Coverage is **analytic**, not sampled: each pixel
  row is subdivided at the heights where an edge crosses a pixel boundary, and between two
  cuts the covered width is linear, so a pixel's coverage is a trapezoid and a path's total
  coverage equals its area. That last property is a test, and it is the one a sampled
  rasterizer cannot satisfy. Paths fill and stroke, with caps, joins, dashes and the miter
  limit; a stroke's parameters travel with the mark that used them. The page's placement —
  the fit, the y-flip and `/Rotate` — is decided in one place, because getting the flip
  wrong renders every page upside down and looks like somebody else's bug.

  Comparisons live here too. **SSIM** in the same windowed form every implementation uses
  (11-tap Gaussian, σ 1.5), plus RMS, max delta and a count of pixels above a tolerance —
  four numbers because they fail in different ways. SSIM alone would pass a page that is
  missing a paragraph of text. A comparison also returns a heatmap, so a failure can be
  looked at rather than only measured.

  **Images** decode and draw. The parts that are easy to get wrong are the parts this does
  most about: `/Decode` maps the *stored range* onto the colour space and is how a file
  writes a negative, packed sub-byte samples are padded *per row* so the byte holding
  sample *n* depends on which row it is in, an `/ImageMask` is a stencil rather than a
  picture, and an `/SMask` is a separate image whose luminance is the alpha. Placement maps
  each pixel *back* through the inverse transformation, which is what makes a rotated image
  come out the right shape instead of a staircase. Colour spaces covered: DeviceGray,
  DeviceRGB, DeviceCMYK, CalGray, CalRGB, ICCBased (by its `/N`) and Indexed. Lab and
  Separation are refused rather than guessed, because neither can be converted without data
  this does not have.

  **Shadings** paint. All four PDF function kinds are here, including the PostScript
  calculator — a small stack machine with the specification's operators, whose
  truth is its own (`0` is false, everything else is true) and whose shifts past the word's
  width are zero rather than a wrap. An axial or radial gradient is sampled *backwards*:
  each pixel is mapped through the inverse of the transformation into the shading's own
  space and turned into a parameter there, which is what makes a diagonal banner work with
  no special case. The gradient's edge is antialiased from the numerical gradient of that
  parameter, so the boundary of a radial gradient — a conic section — needs no formula of
  its own. The disc inside a radial gradient's first circle is filled with the first colour,
  because no circle in the family passes through the family's own centre and the parameter
  there is negative rather than absent.

  **Clips are regions, not boxes.** A clip path is rasterised into a per-pixel coverage
  mask and intersects with any mask already in force, so a diagonal clip is a diagonal and
  two nested clips are their intersection. A mark's rendering depends only on its own record
  — the clip in force when it was created — and not on what the renderer happened to draw
  first, so rendering a page twice gives byte-identical output. A `W n` with an empty path is
  an *empty* clip rather than an absent one, which is the case that separates "nothing is
  drawn" from "everything is drawn".

  A page names a *pattern* and the pattern names a shading, so both shapes are accepted,
  and the pattern's own `/Matrix` composes inside the mark's transformation rather than
  replacing it. Types 1, 4, 5, 6 and 7 are reported as notes rather than painted wrongly.
- **`mangle-cli`** — the headless surface: `info`, `pages`, `check`, `extract`, `save`.
  This is how "what does ManglePDF think of this file?" is asked without a window.
- **`mangle-ui`** — the window shell and the design tokens. Six regions, one grid, one
  stroke weight, a light and a dark theme, and a contrast test that both themes must
  pass.
- **122 hand-authored SVG icons**, lint-clean, with a contact sheet.
- **Docs** — architecture, status, development, testing, PDF quirks, dependencies,
  icons, and the decision records.

## Tests

683 passing, 1 ignored, none failing, no warnings. The ignored one is the Tier-B wild
corpus — a two-hour job, run deliberately with
`cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture`; the two cheap
tests in the same file check the harness itself and run by default. Ten kinds matter:

- **Unit** — one behaviour, stated expectations, including a documented quirk for each.
- **Round trip** — open a file, change it, write it, open it again, compare. This is
  what makes the lossless claim testable rather than asserted.
- **Oracle** — `qpdf` reads what we write. It caught a wrong `/Size`, a cross-reference
  that padded sparse numbering, and a fixture that was wrong rather than damaged. When
  `qpdf` is absent these tests skip and say so; `cargo test` never needs it.
- **What we write, read by something else** — a file this project produces, with its content
  stream compressed, opened by `mutool draw`, `qpdf --check` and `pdfinfo`. This found that
  the compressor emitted raw DEFLATE where `/FlateDecode` names zlib, so a file we wrote
  could not be read by any other program. It turned out to be bidirectional: the decompressor
  had no zlib handling either, so a real-world PDF would not have decoded here either.
- **Corpus** — every Tier-A fixture is opened and checked against its manifest: the
  hash, the page count, whether a repair happened, whether a save reports a dangling
  reference, and that the secrets in `F33` really are in the file.
- **Provenance** — a page is opened, its streams are read, tokenised and run, and every
  mark is checked against the bytes it names. Rewriting a mark's string operand changes
  that mark and leaves every other mark's content alone, which is the property the whole
  content layer exists to provide.
- **Rendering** — a page is rendered and checked against what the page says should be on it.
  Regions are stated as fractions of the page rather than as pixel offsets, so the same
  assertion holds at whatever resolution the oracle is asked to render at; an earlier
  version used fixed pixels and passed only at the resolution it was written for. The
  corpus is rendered at two scales to prove no file hangs or overruns its buffers.
  A red image XObject placed through the resource table is checked, on the same
  scale-independent fractions, and so is an axial shading reached through a pattern — for
  that one the expectation is the gradient's own closed form evaluated at each pixel's
  centre rather than a snapshot, so the assertion states what the gradient should be rather
  than what this renderer last produced.
  Two of those fixtures are about the clip: a diagonal-clip page scores **0.99639** against
  `mutool`, and so does a page whose clip outlives the mark that set it, which is the case a
  per-mark reset got wrong. Both are deliberately asymmetric — nothing is mirrored in either
  axis — because a symmetric page scores above 0.99 whether or not the clip is the right
  shape, so a symmetric fixture cannot see an error sitting off the centre.
- **Fidelity against `mutool`, measured** — the shapes page, the clipped page and the
  diagonal-clip page rendered by `mutool` at 150 DPI (the resolution the acceptance criteria
  name) and compared with SSIM. Shapes **0.99829**, clipped **0.98537**, diagonal clip
  **0.99639**, against a bar of 0.95. Both renderings are flattened onto one background
  first, because `mutool` writes an unpainted page as transparent and this renderer writes
  white paper, and comparing raw buffers would compare conventions rather than renderers.
  Skipped cleanly when `mutool` is absent.
- **Why the clipped page's score went down, and why that is right** — it was 0.99917 when a
  clip was its bounding box and is 0.98537 now. The whole difference is one column of 209
  pixels: `mutool` hard-steps a clip edge onto the pixel grid, snapping a clip at 104.17px to
  105px, while this renderer antialiases it and covers that pixel by its actual share. A sweep
  of the clip edge across one pixel in steps of a tenth of a pixel shows `mutool` producing
  no intermediate value at any of them. The clip's *position* agrees to the pixel: the
  diagonal-clip page's edge sits at the same column on every row checked. Antialiasing a
  clipping path is what the specification permits and what makes a diagonal clip a diagonal,
  so this renderer antialiases it and the disagreement is recorded rather than matched.
- **The diagonal-clip fixture is deliberately asymmetric** — content away from the centre,
  nothing mirrored in either axis — because every earlier fixture was symmetric about the
  page centre, and a symmetric page scores above 0.99 whether or not the clip is the right
  shape. This one scored **0.51652** with the clip as its bounding box and **0.99639** with
  the clip as its path, which is the difference the fixture exists to catch.
- **A clip is a region** — the interpreter records the clipping path, its fill rule and its
  box, and the renderer rasterises the path into a per-pixel coverage mask that is multiplied
  into each fill's own coverage rather than tested against it, so a diagonal clip is a
  diagonal and a circular one is round. Successive clips multiply rather than replace, the
  box is kept as a cheap bound and narrowed whenever the mask is, and a `W n` with an empty
  path is a clip to nothing rather than no clip, which is what the specification says.
- **The clip's region decides visibility, not its box** — a triangle's corner just outside
  the hypotenuse is paper rather than ink, a disc leaves all four corners bare with an
  antialiased edge between them, two successive clips leave only their intersection, a
  rectangle clip is still exact and still costs one branch per pixel, an empty clip draws
  nothing at all, and `reset_clip` brings the whole page back with the mask and the box
  together.
- **The text model against another renderer** — 400 cases through `mutool draw -F trace`,
  compared glyph position by glyph position, failing loudly rather than skipping when the
  renderer is absent so that it cannot pass by having nothing to compare against. This is
  what found a `TJ` kern displacing the wrong glyph, kerns ignoring a horizontal scale, and
  the font size being applied twice.
- **Composite fonts, against `mutool` too** — a `/Type0` font with `/Identity-H` and an
  embedded `/CIDFontType2` is rendered and compared at 0.99225 SSIM. The fixture is
  deliberately asymmetric: three different CIDs, three different declared widths, and a
  `/W` array that mixes the listed and the ranged form. A symmetric fixture would score well
  against a renderer that drew the *wrong* glyphs, because the error would be a mirror image
  of itself.
- **CFF outlines, against `mutool`** — a page whose `/FontFile3` is a whole OTF with a `CFF `
  table is rendered at 150 DPI and compared at **0.98654** SSIM, against the same bar of
  0.95, having covered 3 333 946 ink pixels where `mutool` covered 3 330 782. The fixture is
  deliberately asymmetric: three lines of different lengths at different sizes and starting
  positions, and a filled grey rectangle beside them, nothing mirrored in either axis. Three
  lines rather than a word is the point — the same letters at three sizes means one wrong
  advance shows up as drift on the third line only, which a symmetric page cannot show.
- **Every glyph of every CFF font on the machine, against `ttf-parser`** — `ttf-parser` has
  its own CFF interpreter, written from the same specification by different hands, so
  walking a font with both and comparing every coordinate of every segment is a check on
  correctness rather than on consistency. 28 347 outlines agree and none differ. This is what
  found the defect that mattered most: a charstring's curve operators give three points as
  offsets **from the point before each of them** — the first from the pen, the second from
  the first, the third from the second — and reading all three from the pen produces a
  closed, plausible, *wrong* glyph rather than an error. Every letter came out too narrow and
  every curve too flat: 0.99 SSIM for a TrueType face on the same code, 0.82 for this one.
  The widths are cross-checked the same way, against the same font's `hmtx` — 10 537 agree,
  none differ — and the subroutine bias is tested at 107, 1131 and 32768 *and at 1239, 1240
  and 33 900 subroutines*, because the boundaries are where an off-by-one lives and one
  sample inside a range cannot see one.
- **Every glyph of every Type 1 font on the machine, against the same face in OpenType** —
  the 28 URW and Nimbus faces are installed twice on a machine that has Ghostscript: once as
  bare PFA programs, which is what a `/FontFile` carries, and once as OpenType/CFF. Both
  readers are ours and both were written from a specification, so a disagreement between them
  is a disagreement between two readings of the same outlines. All 28 609 glyphs of six fonts
  walk with no refusal and no coordinate outside four ems, and 376 glyphs across four fonts
  agree with their twins. This is what found that `hvcurveto` and `vhcurveto` take four
  operands of which the *fourth* is the endpoint's x for the vertical one and its y for the
  horizontal one — reading them the other way round swaps the two and turns every round letter
  into a shape that closes and does not match. It cost 0.15 of SSIM on a page of ordinary
  text. The same test found that `hsbw`'s first operand says where the outline sits relative
  to the pen, which the charstring's own coordinates do not include.
- **Widths against two other renderers** — the standard fonts' metrics are checked by
  measuring where a real renderer puts each glyph, not by reading the table back. One
  transcription error was found this way: `fraction` in Helvetica had the width of the URW
  clone, 278, where Adobe's own metrics say 167, and poppler agrees with Adobe. Every width
  in every table was then re-checked against the metrics files, and all 1043 agree.
- **Encodings, against the specification's own tables and against `mutool`** — the three base
  encodings and `PDFDocEncoding` are transcribed from Table D and Annex D.2, and each was then
  checked against something on the machine rather than trusted: StandardEncoding against
  Ghostscript's own decoding table entry for entry; WinAnsi against the construction in
  Ghostscript's init file and against `pdftotext`'s reading of the same encoding; MacRoman
  against the machine's `mac_roman` code page and poppler's `MacRomanEncoding`, which agree on
  every code from 32 to 225. The glyph names come from Adobe's Glyph List as shipped with
  `read-fonts`, which is a machine source and not a transcription — so the Greek and Cyrillic
  names a `/Differences` array really uses are here in full (246 `afii` names, the 111-name
  Greek and Coptic block) rather than restricted to Latin. Two tests earn their keep: every
  name any table can produce is either a width in a standard-14 table or on an explicit
  known-gap list, both ways, so a transposed entry fails and the list cannot rot; and every
  name in every table resolves through the AGL, so a typo becomes a failure where the typo is.
  The rendering oracle is a page whose font `/Encoding` remaps eighteen codes, four of them
  inside the Latin-1 range where WinAnsi and MacRoman disagree completely, at **0.98780**
  SSIM. The fixture is deliberately asymmetric and its remaps are not a permutation of each
  other, which catches three separate mistakes: ignoring the array, stopping at the end of the
  base, and transposing two codes. Renaming `/Differences` in the file and re-scoring drops it
  to 0.895, so the test does see the thing it is for.

## Gaps, in the order they block

1. **A clip's paths are capped at 32.** A page nesting more than that drops its outermost
   paths and falls back to box-only culling, so a deeply nested clip loses an antialiased
   edge. The cap exists because each path costs a full coverage rasterisation, and the
   honest fix is to composite the paths in a shared sweep rather than to raise the number.
2. **The standard fourteen and Type 3 do not draw.** A glyph is filled from an embedded
   program: a `/FontFile2` through a table walk, a `/FontFile3` through a Type 2 charstring
   interpreter, compared against `mutool` at 0.962 and 0.987 SSIM respectively. What is still
   missing is the standard fourteen, which have no program at all, and Type 3, whose glyphs
   are content streams rather than outlines. A page using one is reported rather than drawn
   blank. Composite (Type 0) fonts draw too, and are counted as done below rather than
   here. A Type 1 (`FontFile`) program is a CFF table holding Type 1 charstrings, which is a
   different language from the Type 2 this reads, so a font that says so is refused with a
   reason.

   This is the top item on the list by corpus weight, not by difficulty. **355 of the wild
   corpus's 542 pages are blank because of it.** A `Type1` simple font whose descriptor
   carries a bare CFF (`/FontFile3` with no `sfnt` wrapper, so no `cmap`) needs its charset
   to get from a character code to a glyph — `code → /Encoding /Differences name → charset
   name → GID` — and the charset is not read, so the font is refused rather than guessed at
   and every character in it goes undrawn. Another 67 pages name a standard fourteen font
   that is not embedded. The charset is ISO 10581 §5.2: three formats and the 391-entry
   Standard Strings list. See D7 in `docs/known-diffs.md` for the measurements.
3. **Composite fonts read two-byte codes, and the mark says so.** A `/Type0` font's
   character codes are two bytes, so a string is split into codes rather than bytes and the
   pen advances by the width of each code. Two things had to change together for that to be
   right rather than merely different. The width lookup reads the descendant font's
   run-length `/W` — `c [w1 w2 …]` and `c_first c_last w`, with `/DW` (1000) for a code no
   run covers — because that array is indexed by code and not by position from a
   `/FirstChar`, which is what a simple font's `/Widths` is. And a `Mark::Glyphs` now carries
   the character codes and whether they are two bytes, so a renderer is told the width of a
   code instead of inferring it from a byte count: the codes are the same numbers in both
   cases, and only the font dictionary says how wide they are. The renderer looks a composite
   code up through the font's (3,0) subtable, falling back to the code as a glyph number,
   which is what a CID font with no `/CIDToGIDMap` means. A simple font takes none of this
   path: its codes are one byte, its widths are a flat run, and the only cost is one branch
   the common case never takes. `/ToUnicode` is not read — it belongs to text extraction,
   not to drawing — but the mark now carries what extraction will need: the codes, the font
   they were shown in, and the span of the bytes they came from.
4. **The font size is applied once, in one place, and the model is tested against another
   renderer.** The text matrix is measured in ems and carries no font size; the size enters
   when a glyph is drawn. The model is pinned by a differential test over 400 cases against
   `mutool draw -F trace`, which is how the last three text bugs were found and how the
   first attempt at this model was shown to be wrong: the specification's `Tc` and `Tw` are
   added raw, not divided by the size, and getting that backwards matched 29 of 400 cases
   where the implemented model matches all 400.
4. **JBIG2 and JPEG 2000 have no decoder.** An image needing one is reported by name rather
   than drawn as a blank rectangle, because a page with a conspicuous hole is a bug report
   and a page with a missing photograph is a wrong answer. CCITT does decode, through the
   filter crate.
5. **Tiling patterns and mesh shadings draw nothing.** A shading names one of types 1, 4, 5,
   6 or 7, or a pattern needs a tiling loop, and each is reported as a note against the mark
   rather than skipped silently, so a page that used one is visibly incomplete instead of
   quietly wrong. Images and axial/radial shadings now draw.
6. **The Inspector does not exist.** `mangle-ui` draws the region; nothing populates it
   from the marks the content layer produces.
7. **An object that came out of an object stream cannot keep its original bytes**,
   because it had none: it was compressed with everything else in its container. A full
   save writes it as a direct object, which every reader accepts but which is a
   re-serialisation rather than a copy.
8. **No signature writing.** `ByteRange`, CMS and DocMDP all still have to be built.

## Known limitations in the finished layers

- The inflate bomb ceiling is a ratio cap. That is provably safe for deflate, which
  cannot exceed about 1032:1, but it is not a defence against a merely *large* stream.
- The crypt filter is consulted per object now, so `/StmF` and `/StrF` may differ and
  `/Identity` is honoured. What is still missing is encryption *writing* for revisions
  5 and 6: `/U`, `/O`, `/OE`, `/UE` and `/Perms` have no writer.
- `saslprep` is a reduced implementation: it maps the C.1.2 spaces, drops part of
  Table B.1 and applies NFKC, but it does not perform the RFC 3454 prohibition checks or
  the bidirectional check. A password using those characters is handled differently
  from one that is not.
- Object-stream writing is not implemented, so `WriteOptions::use_object_streams` has no
  effect. The defaults deliberately produce the classic layout, which every reader
  accepts.
- A full save copies the original header version, but the cross-reference and trailer are
  necessarily rebuilt: a rewrite moves every object, so their offsets change. Every
  *object* keeps its bytes, which a test proves across the whole corpus.
- `mangle-filters::deflate` writes a **zlib** stream (RFC 1950) — the two-byte header, the
  deflate data, the Adler-32 of the original — which is what `/FlateDecode` names, so a
  file this project writes is readable by another program. `deflate_raw` is the bare RFC 1951
  form for a caller that wants it, and `inflate` accepts both, because files in the wild
  carry a wrapper and, occasionally, do not. `qpdf --check`, `mutool draw` and `pdfinfo` all
  read a `/FlateDecode` stream we write; the same file written as a bare deflate stream is
  rejected by all three. Every other filter we produce still needs the same checking before
  the writer ships.
- A page's pixel buffer rounds *up* to a whole pixel, so a page of 208.33 pixels gets the
  209 rows it needs and its last third of a pixel is not clipped. A value that binary
  floating point has nudged a hair above an integer counts as that integer before the
  ceiling, so a US-Letter page is 1275×1650 at 150 DPI rather than 1275×1651. See D3 in
  `docs/known-diffs.md`.

## What the Tier-B wild corpus found

The corpus is 77 hard files from pdf.js, PDFBox, NIST, the IRS, the USGS and arXiv, pinned
by SHA-256 in `corpus/wild/MANIFEST.toml` and measured against `mutool draw -r 150` and
`pdftotext`. One run, 77 files and 542 pages, produced **two clusters** and almost nothing
else — which is the point of having it.

**34 files do not open.** 29 of them say `dangling reference: the page tree root is
missing`, 4 say `no catalogue`, and 1 is `the document is encrypted: incorrect password`.
So this is one dominant bug and a long tail, not 34 separate ones: the cross-reference
*stream* path cannot be used, the reader falls back to scanning for `N G obj` headers, and
an object stream hides `/Pages` from a scan. Between them those 34 files cover live
government forms (IRS 1040 and W-4, NIST FIPS 197 and SP 800-88), two 1:25000 USGS topo
sheets and every Adobe InDesign output in the set.

**80 pages could not be compared at all, from one off-by-one.** They rendered 1275×1651
where `mutool` renders 1275×1650 — 70 of them exactly that pair, the rest on other page
sizes — and `compare()` refuses to compare images of different sizes. 70 of the 80 are a
US-Letter page, one arXiv paper has 23 Letter pages and the other has 12, so the corpus
was measuring almost no LaTeX at all. The cause was `ceil()` applied to a product floating
point had already put a hair above the integer: `792.0 * (150.0/72.0)` is
`1650.0000000000002`. **Fixed** — see D3.

The fix matters more than its size suggests, because the two clusters interact. A page that
is one pixel too tall is not scored slightly worse; it is *refused*, so 80 pages produced no
SSIM at all and the file looks like a renderer that works. Now that the sizes agree those
pages are measured: `gov__arxiv-1206.5537.pdf` page 1 goes from *not measured* to **0.7915
SSIM** against `mutool`. A full re-run of the corpus is wanted to re-baseline the median; it
takes two hours.

### The blank-page cluster is fixed, and it was the smaller of the two defects

Every one of the 542 pages was rendering with `marks: 0` and no ink, because `render_page`
handed the content interpreter the *encoded* content stream (D1). Fixed — one line, and a
fixture that inflates its content so the suite can never again be silent about it.

Re-measured with the fix in place, the corpus says:

| | pages |
|---|---|
| draw something | **103** |
| still blank | **439** — 422 of them because of a font, 17 for other reasons |

So D1 was the cause of the blankness on 19% of pages and on the other 81% it was *hiding* a
second and much larger defect behind silence. That defect is fonts, and it is now named,
measured and written down as D7: 355 pages carry a `Type1` font whose `/FontFile3` is a bare
CFF table, where a character code reaches a glyph through the CFF charset, and nothing reads
the charset. Another 67 name a base-14 font that is not embedded at all.

**The arXiv page's SSIM did not move, and that is the right result.** It is 0.791534 before
and after, because the page is blank both ways for two different reasons — before because
the content was never decoded, after because all seven of its fonts refuse. A blank page
against a page with 5.4% ink scores 0.7915 whatever the reason for the blankness is.

The reporting is the durable part of the fix. Every filter that goes wrong now says so in
`render.notes`, naming the filter and which of the page's `/Contents` streams it came from,
and bytes that are still encoded are refused rather than handed to the operator table. That
is what turned "439 pages are blank" from a two-week question into a table.

**No SSIM figure in `docs/known-diffs.md` should be read as a fidelity claim until D7 is
fixed**, because a renderer that cannot draw a single glyph cannot say anything about
fidelity — and a blank page compared against a nearly-blank oracle scores 0.9995.

The corpus test asserts nothing and the report is the output — the reasoning is at the top
of `crates/mangle-render/tests/wild_corpus.rs`. A threshold on a document nobody has read
yet turns the first surprise into a permanent red build, and the response to a permanent red
build is to raise the threshold, which is the one thing the corpus was for.
