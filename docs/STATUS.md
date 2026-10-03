# Status

Where the work actually is. Updated whenever a milestone moves.

A word about the tests here, because it is the pattern rather than any single finding. Five
of the real defects this project has had were invisible to its own suite and were found by
comparing against something outside: a page rendered at scale squared, a clip transformed
twice, a filled path transformed twice, a test helper missing an absolute value, and a
compression format that only this project could read. Each was masked by the same thing —
fixtures written to be simple, and therefore symmetric. A symmetric page renders correctly
even when it is rendered wrongly, so the shape of a fixture decides how much of a renderer's
arithmetic it actually exercises. The two path defects were masked the same way twice over:
the one test that named the doubled transformation used a *clip*, which was installed from the
page placement alone and therefore already right, and its fill was exactly its clip — so the
test passed with the bug and would have passed with the fix. A gradient function is the same
shape of mistake in the other direction: `C0 + x^N·(C1 − C0)` and `C0 + x^N·C1` are the same
number when `C0` is zero, and every fixture in this repository that exercises one writes
`C0 [0]`.

## Milestones

| M | Deliverable | State |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen, window shell, icon pipeline | **done** — every Gate 0 check passes; the window draws the six regions from tokens and nothing else |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector | **mostly done** — everything except the Inspector. See "Gaps" below |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | **partly done** — the tokeniser, the operator table, the graphics state and the interpreter exist, and the rasterizer now turns a page's paths, its embedded-TrueType glyphs and its embedded-CFF glyphs into pixels with analytic coverage. Images and shadings draw, a shading pattern used as a fill colour draws per pixel, and tiling patterns and `Symbol`/`ZapfDingbats` still draw nothing |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | **partly done** — the four PDF function kinds, axial and radial shadings and the device colour spaces paint, and TrueType, composite, CFF and Type 1 outlines all draw. Twelve of the standard fourteen draw from bundled metric-compatible faces when a document names one without embedding it. Type 3, mesh shadings, tiling patterns and transparency do not |
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
  widths now agree with the metrics files.

   **A standard font that declares no `/Widths` now answers from those tables** —
   `metrics::standard_run` — and this was the *larger half* of what an unembedded standard-14
   font cost, larger than the outlines, because it moved every character after the first.
   `Resources::from_dict` recorded a width run only when `/Widths` was present, and the
   standard fourteen have none by definition, so such a font advanced *every* glyph by the
   500-unit default: the right glyph shapes in the wrong columns, which reads as text set
   slightly badly rather than as text that was never drawn. The run is the same built-in
   table indexed by **code** rather than by glyph name, and read through the font's *own*
   `/Encoding`, because the width of 0x92 is a question about the document rather than about
   Helvetica. A `/WinAnsiEncoding` font, a `/MacRomanEncoding` one and one with a
   `/Differences` array each therefore get their own run, and a name with no table gets none,
   so the caller's refusal stands rather than being replaced by a width from the wrong font.

   **Outlines** come from embedded TrueType through
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

  - **A charset this cannot walk resolves no code.** A name-keyed CFF's `charset` *is* read,
    in all three of its formats and over both the 391 Standard Strings and the font's own
    String INDEX, so a bare `/Subtype /Type1C` font resolves its codes like any other. What is
    still refused is a charset that is *present and unreadable*: one of the three predefined
    charsets, whose offsets are not a table at all but a statement that every glyph has a
    standard name. That case says so on the page rather than drawing nothing. A name the
    charset lacks is a reason rather than a guess, because on a subsetted font there is always
    a nearby glyph and drawing that one puts a character nobody asked for in place of one
    they did.
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
- **`assets/fonts/` — the substitute-face mechanism.** Twelve Liberation faces (Regular,
  Bold, Italic, BoldItalic of Sans, Serif and Mono) plus the OFL text, bundled because a
  document naming one of the standard fourteen *without embedding it* — which is most of the
  LaTeX and office output in the wild, because those producers assume the reader has the
  font — is naming a font, not naming nothing. Refusing it leaves a page with a hole where
  the text was, which is a worse answer than drawing the text in a face that agrees about
  every width. Liberation Sans is metric-compatible with Helvetica, Serif with Times and
  Mono with Courier, so a line keeps the breaks and a column keeps the width the producer
  laid it out against.

  `mangle_font::substitute` maps a `/BaseFont` to a face through the **same** name-splitting
  the width tables use, so a subset prefix (`ABCDEF+Helvetica-Bold`) and the older comma
  spelling (`Helvetica-Bold,Italic`) both resolve and there is no second name parser here
  that can disagree with the first. The faces are unmodified TrueType (`glyf`) read by
  `from_true_type`, so no CFF or Type 1 path is involved, and `OS/2 fsType` is respected at
  runtime — a face that forbids embedding is reported to the user, never overridden.

  The substitution is **reported, not silent**: the page carries a note naming the face that
  stands in, because the outlines on the paper are that face's and not the original's, and
  that is the one fact about the picture a user cannot read off it. `Symbol` and
  `ZapfDingbats` have no metric-compatible substitute and stay refused — inventing a stand-in
  for a symbol font would silently put the wrong glyphs on the page. Three rough edges are
  recorded in `PLAN.md`.

  **The cost is on the record:** `include_bytes!` puts the twelve faces — 4.4 MB, 4 360 440
  bytes — into every binary that links `mangle-font`, with no option to leave them out. That is
  the deliberate trade for a renderer that never has to ask the machine for a font.
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

  **A pattern colour is not one colour.** A `/Pattern` colour space means the operands named
  a pattern resource rather than a colour value, so there is nothing for a colour-space
  converter to convert: a `/PatternType 2` pattern's colour changes with position across the
  shape being filled and has to be evaluated per pixel where the fill lands. It is evaluated
  by the *same* evaluator `sh` uses — one inverse mapping, one antialiased edge, one set of
  rules — because two evaluators that agree today drift apart tomorrow, and the symptom is a
  gradient that looks right in a `sh` and wrong in a fill. A test asserts that the two produce
  identical pixels, which is the property that says they are one evaluator. The pattern's
  `/Matrix` maps into the page's *default* user space and not through the mark's own
  transformation: a `cm` that moves the shape leaves the pattern where it was, which is the
  specification's rule, `mutool` agrees with, and the one a test pins by watching a mask
  placed under a scaling `cm` come out with the gradient the page asked for.

  This draws an image mask painted in a pattern colour, which is what
  `pdfjs__issue13372.pdf` is: a portrait whose every pixel is a gradient rather than a
  sample. A `/PatternType 1` tiling pattern is **reported by name** rather than approximated
  with a single cell, and a shading whose colour space is `/Separation`, `/DeviceN` or
  `/Indexed` is reported rather than read as the grey its one-component function looks like.
  A pattern used as a *stroking* colour, or as the colour of text, is reported and not
  painted: this is a fill feature, and `Colour::to_rgba` now says a `Pattern` space is not a
  colour at all rather than handing a caller black, which is what a stroke in one used to get
  silently. A bare `/Separation` **tint** (`0.5 scn` in a separation space rather than a
  pattern name) is a different question and still takes its pre-existing fallback — the
  alternate colour, or black.
- **`mangle-cli`** — the headless surface: `info`, `pages`, `check`, `extract`, `save`.
  This is how "what does ManglePDF think of this file?" is asked without a window.
- **`mangle-ui`** — the window shell and the design tokens. Six regions, one grid, one
  stroke weight, a light and a dark theme, and a contrast test that both themes must
  pass.
- **122 hand-authored SVG icons**, lint-clean, with a contact sheet.
- **Docs** — architecture, status, development, testing, PDF quirks, dependencies,
  icons, and the decision records.

## Tests

792 passing, 1 ignored, none failing, no warnings. The ignored one is the Tier-B wild
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
  than what this renderer last produced. A *pattern as a fill colour* is checked the same
  way, at both ends of the fill and at points between, and its most important test is not a
  colour at all: the same gradient filled as a shape and painted by `sh` through a clip must
  produce **identical pixels**, because that equality is what says the two are one evaluator
  rather than two that happen to agree. An image mask in a pattern colour draws; a tiling
  pattern is reported by name; a shading in a `/Separation` colour space is reported rather
  than read as the grey its one-component function looks like.
  A filled path under a `cm` is checked against the closed form rather than a proportion of
  the page: `50 0 0 50 10 10 cm` over `0 0 1 1 re` must cover device pixels `10·scale` to
  `60·scale` on both axes and nothing else, at three scales, and `30 0 0 10` must give a
  rectangle three times as wide as it is tall rather than the nine-to-one one a second
  application of the transformation would give. The same path with no `cm` is pinned
  separately, because that is the case every other fixture in the file is and the one a fix in
  the wrong place would leave passing. An image and a glyph under the same kind of `cm` are
  compared against the same mark with no `cm`, which is what says they were not moved by it.
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

### An ignored test is not a failed test

G0.5 used to fail on any `#[ignore]`d test, and the Tier-B corpus run — ignored on
purpose, because it takes two hours — failed the gate because of it. That was the wrong
rule: an ignored test is not run, so it is not evidence that anything works and it is not
evidence that anything is broken. Failing on it only forces the choice between a gate
that lies and a suite that lies.

The gate now reads what `cargo test` actually reported, per test binary, and judges on
passed and failed. Ignored and filtered-out tests are counted and reported rather than
failed or dropped in silence:

```text
G0.5  pass  fmt, clippy, tests and release build are clean
        cargo test: 722 passed, 0 failed, 1 ignored, 0 filtered out over 39 test binaries
        1 ignored test(s) were not run, so they are excluded from the judgement rather
        than counted as failures: none
```

Nothing else got weaker. A failed test still fails the gate, by name, and a run that
reports no test binary at all fails it too — "nothing ran" is not a clean result. The
parsing and the decision are unit-tested in `xtask/src/policy.rs` against recorded
`cargo test` output, so checking the rule costs no cargo run.

## Gaps, in the order they block

1. **A clip's paths are capped at 32.** A page nesting more than that drops its outermost
   paths and falls back to box-only culling, so a deeply nested clip loses an antialiased
   edge. The cap exists because each path costs a full coverage rasterisation, and the
   honest fix is to composite the paths in a shared sweep rather than to raise the number.
2. **Type 3, `Symbol` and `ZapfDingbats` do not draw.** A glyph is filled from an embedded
   program: a `/FontFile2` through a table walk, a `/FontFile3` through a Type 2 charstring
   interpreter, compared against `mutool` at 0.962 and 0.987 SSIM respectively. What is still
   missing is Type 3, whose glyphs are content streams rather than outlines, and the two
   standard faces Liberation has no equivalent of. A page using one is reported rather than
   drawn blank. Composite (Type 0) fonts draw too, and are counted as done below rather than
   here. A Type 1 (`FontFile`) program is a CFF table holding Type 1 charstrings, which is a
   different language from the Type 2 this reads, so a font that says so is refused with a
   reason.

   **This is done.** A `Type1` simple font whose descriptor carries a bare CFF
   (`/FontFile3` with no `sfnt` wrapper, so no `cmap`) gets from a character code to a glyph
   by `code → /Encoding /Differences name → charset name → GID`, and all three steps work.
   That was **355 of the wild corpus's 542 pages**, blank because the charset was not read.

   Three things about it were not obvious and are worth recording, because each was a way of
   reading good bytes into a wrong answer:

   - **No charset format has a count.** All three are a format byte and then entries, and the
     count is the glyph count from the CharStrings INDEX. A reader that looks for a `u16`
     count reads the first entry as a header — for format 0, usually a count of zero, so the
     charset comes back empty while the bytes are fine.
   - **A String INDEX entry has no length byte.** The length is the INDEX's own offset
     arithmetic. Across the 84 String INDEX entries in the corpus not one begins with a byte
     equal to its own length.
   - **The first glyph a run names is glyph 1**, because `.notdef` is glyph 0 and no format
     lists it.

   The 391 Standard Strings are transcribed and were checked against two independent sources
   that agree on all 391: Ghostscript's `CFFStandardStrings` pseudo-encoding, whose source
   annotates each name with its SID, and the Adobe Glyph List toolchain's
   `cffStandardStrings`. The 355-page refusal was replaced by a standing cross-check against
   `fontTools` over the whole corpus — `crates/mangle-font/tests/charset_real.rs`, which
   hands the same bytes to both readers and asserts they agree on **every glyph name**
   (currently 74 charsets and 1874 names, no disagreements). Without it the three layout
   mistakes above would all have shipped, because the hand-built test fonts were written to
   match them.

   What is left of D7 is the other half: 67 pages name a standard fourteen font that is not
   embedded at all. See D7 and D8 in `docs/known-diffs.md` for the measurements.

   **That half is done too**, and it was two fixes rather than one. The outlines came from
   the bundled metric-compatible faces (`assets/fonts/`, see "What exists"), and — the larger
   half — the *widths* came from the built-in tables rather than from the 500-unit default,
   because `Resources::from_dict` only recorded a run when `/Widths` was present. Drawing the
   glyphs without that would have put every character after the first in the wrong column and
   scored *worse* against the oracle, so the two had to land together.

   Measured against `mutool`, the two corpus files this moved most:

   | file | SSIM before | SSIM after | ink before | ink after | oracle ink |
   |---|---|---|---|---|---|
   | `pdfbox__data-000001` | 0.93127 | **0.94703** | 448 654 | **477 471** | 483 043 |
   | `gov__arxiv-1206.5537` | 0.87844 | **0.88414** | — | — | — |

   `pdfbox__data-000001` now covers 98.9% of the oracle's ink. Neither number is close to
   0.99, and the reason is recorded in `PLAN.md`: this closes the pages whose *only* obstacle
   was a font the document did not embed, and a good deal of the rest of the corpus names
   `TimesNewRomanPSMT`, `CourierNewPSMT` and `Arial*`, which `metrics.rs` used to refuse
   because they are not standard-fourteen names. **Those are mapped now** — see "Metric-compatible
   aliases" below. `HelveticaNeueLTStd-*` still refuses, and is meant to: Helvetica Neue is not
   metric-compatible with Helvetica, so an alias for it would not be an approximation but a
   wrong number. A full corpus re-run is wanted and takes two hours.
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
5. **JBIG2 and JPEG 2000 have no decoder.** An image needing one is reported by name rather
   than drawn as a blank rectangle, because a page with a conspicuous hole is a bug report
   and a page with a missing photograph is a wrong answer. CCITT does decode, through the
   filter crate, and its two-dimensional path is now checked against `libtiff` — the
   expectations are rasters `libtiff` decoded, not assertions derived from the specification.
6. **Tiling patterns and mesh shadings draw nothing.** A shading names one of types 1, 4, 5,
   6 or 7, or a pattern needs a tiling loop, and each is reported as a note against the mark
   rather than skipped silently, so a page that used one is visibly incomplete instead of
   quietly wrong. A *shading* pattern as a fill colour does draw, per pixel; the tiling loop
   is what is missing, and a note about a fill in a tiling says which pattern type it did not
   draw, because one cell of a tiling is a texture that looks plausible and is wrong
   everywhere. The type-2 function's own rule was a separate matter, recorded as D10 and
   since fixed against ISO 32000-1 Table 42.
7. **The Inspector does not exist.** `mangle-ui` draws the region; nothing populates it
   from the marks the content layer produces.
8. **An object that came out of an object stream cannot keep its original bytes**,
   because it had none: it was compressed with everything else in its container. A full
   save writes it as a direct object, which every reader accepts but which is a
   re-serialisation rather than a copy.
9. **No signature writing.** `ByteRange`, CMS and DocMDP all still have to be built.
10. **A stroke's width is not a stroked ellipse.** The width now scales with everything the
    geometry scales with — the content stream's `cm` at interpretation, the page placement at
    draw time — so a stroke thickens when the page is drawn larger, and the missing factor
    D9 measured is gone: `2 w` on `0 0 10 10 re` under `4 0 0 4 50 50 cm` was 48 device
    pixels across at scale 1 and 88 at scale 2, and is 96 now, which is what `mutool` draws. A
    *non-uniform* `cm` is the part still standing: the true stroke is the ellipse the matrix
    gives a circle, and this renderer has one width rather than a pen that can be elliptical,
    so it takes `sqrt(|det|)`, the geometric mean of the two axis scales. For `4 0 0 2` with
    `2 w` both oracles draw 48 by 24 device pixels — the exact ellipse — where this draws
    45.7 by 25.7. See D9 and `docs/known-diffs.md`.

    **What it bought, measured against `mutool` at 150 DPI** on the two corpus pages with the
    most stroked ink, which is where a missing factor showed:

    | file | SSIM before | SSIM after | our ink before | our ink after | oracle ink |
    |---|---|---|---|---|---|
    | `gov__irs-f1040` | 0.93970 | **0.97348** | 45 501 092 | 50 689 496 | 52 616 027 |
    | `gov__irs-fw4` | 0.95949 | **0.97742** | 40 588 966 | 43 846 558 | 44 132 346 |

    `gov__arxiv-1512.03385` moved 0.94453 → 0.94472, and three files this renderer draws no
    strokes on measured identical before and after — `pdfbox__PDFA3A` at 0.99923,
    `pdfbox__simple-openoffice` at 0.99963, `pdfbox__openoffice-test-document` at 0.99998 —
    which is what says the change is confined to strokes. A full 542-page re-run is still
    wanted.
11. **A dash pattern's lengths scale with the page, and its gaps are paper.** Both halves of
    this were wrong at once, and the second hid the first: `stroke_polygon` received every
    on-run and off-run from `walk_dashes` and **stroked both**, so any pattern drew as one
    solid line, and the lengths were user-space lengths that nothing turned into pixels. A
    `[6 3] 0 d` line was therefore a solid 160-point run at 72, 144 and 288 DPI alike.

    Now the gaps are paper and the lengths scale, and both oracles agree with the result to the
    pixel — `mutool draw` at 72, 144 and 288 DPI gives on-runs of 6, 12 and 24 pixels, and so
    do we; `pdftoppm` agrees at 72. The same holds for `[6 3] 2 d`, where the two-unit phase
    leaves a four-pixel first run at 72 DPI and an eight-pixel one at 144, and for the
    zero-length entry `[6 0 3 4] 0 d`, which is nine of ink and four of paper at 72 DPI in all
    three renderers.

    **The scaling is in `page.rs`, beside the geometry, not in the interpreter** — the
    interpreter knows the content stream's `cm` and no more, so a pattern scaled there would
    carry one factor where the canvas's zoom is not. The two factors are multiplied exactly as
    `device_line_width` multiplies them for a width. **The phase is scaled by the same factor
    as the lengths**, which is what keeps it a phase: it is a distance into the pattern measured
    against the pattern's own total, so scaling both leaves the fraction named unchanged, and
    scaling only the lengths would move every dash along the line as the page is zoomed. **An
    array that sums to zero is drawn solid** — `walk_dashes` returns before it divides by the
    total — where both oracles draw nothing; that one is recorded in D11b rather than closed.

    **What it bought, measured against `mutool` at 150 DPI**, page 1, on the corpus files that
    carry a dash in their content:

    | file | SSIM before | SSIM after | our ink before | our ink after | oracle ink |
    |---|---|---|---|---|---|
    | `gov__irs-f1040` | 0.97348 | **0.97408** | 50 689 496 | 50 572 581 | 52 616 027 |
    | `gov__arxiv-1512.03385` | 0.94472 | 0.94472 | 28 689 444 | 28 689 444 | 29 188 929 |

    Both are small movements, and honestly read: `gov__irs-f1040`'s page 1 gains 0.0006 SSIM
    while its ink falls 116 915 further short of the oracle's, and `gov__arxiv-1512.03385`'s
    page 1 does not move at all because its dashes are on another page. The corpus measurement
    is weak here for a reason worth stating rather than hiding — **six of the seven corpus
    files that contain a dash pattern draw almost no dashed ink on page 1**, so a single-page
    score barely registers a change confined to dashes. What the oracle comparison above is for
    is the direct check, and it is exact.

    One further find, recorded because it was a page that drew nothing at all:
    `pdfjs__issue13325_reduced` put **zero ink** on the page — 11 marks recorded and nothing
    composited. That was not a dash defect, and it is now fixed; see
    [the entry below](#a-stroke-painted-in-a-pattern-was-dropped-and-the-page-was-blank).

## A stroke painted in a pattern was dropped, and the page was blank

The zero-ink page above is fixed, and the cause was not one of the four a blank page usually
turns out to be. It was not a clip (the clip on every mark is the whole mediabox), not a
placement (our CTM for the first stroke is `1 0 0 1 299.3702 566.1895`, which is `mutool
trace`'s `1 0 0 -1 299.3702 275.7005` with the page's height taken off), not an alpha of zero
(`/GS0` is `ca 1 CA 1`), and not content hiding in an XObject (the one form, `/Fm0`, is
`0 TL q Q` — empty).

The renderer said what was wrong, seven times, once per stroke:

```
a stroke colour in Pattern could not be converted, so the outline was not drawn
```

The page's seven strokes are set with `/CS1 CS /P0 SCN`, where `/CS1` is `[/Pattern]` and
`/P0` is a `/PatternType 2` shading. **`SCN` naming a pattern as the stroke colour is ordinary
PDF** — it is the only operator that can, since `G`, `RG`, `K` and `g`/`rg`/`k` cannot name a
pattern resource at all — and a gradient rule is how a design tool draws a rule. The fill arm of
`draw_mark` already read a `Pattern` fill and evaluated its shading per pixel. The **stroke** arm
asked a colour converter for one colour, and `Colour::to_rgba` answers `None` for a `Pattern`
space by design, because a pattern is not a colour. So every mark that carried the page's ink was
dropped, and a page whose ink is entirely patterned strokes renders blank.

The fix is one outline and two paints. `Device::stroke_outline` now builds the stroke's outline —
dashes, caps, joins — and returns it per subpath; `Device::stroke_polygon` fills it with one
colour as before, and `fill::stroke` fills the same outline through `fill::polygon` with a
`FillColour` that may be a shading. One piece of code computes the outline, so a patterned stroke
cannot come out dashed differently from a flat one, and
`a_pattern_stroke_covers_exactly_where_a_one_colour_stroke_does` pins that by comparing the two
pixel for pixel.

Page 1 of `pdfjs__issue13325_reduced.pdf` against `mutool draw -r 150`, 1241×1754:

| | SSIM | RMS | pixels above tolerance | our ink |
|---|---|---|---|---|
| before | 0.99644 | 4.85033 | 2 643 | **0** |
| after | **0.99741** | **3.69345** | **2 466** | **3 439** |
| `mutool` | — | — | — | 2 743 |

Read the ink column and not the SSIM column. The page is **99.9% white**, so a blank page and a
page with all of its marks drawn score 0.996 against each other, and an SSIM that moves in the
fifth decimal is what a whole page appearing looks like here. Zero ink became a page that draws.

What is left on the page is a different defect, and chasing it is how it turned up.
**A two-point dash run drawn in the negative x direction is a bowtie**, hollow in the middle,
while the same dash drawn left to right is solid:

```
80 50 m 20 50 l S    ....................034689986430............034689986430...
20 50 m 80 50 l S    ....................000000000000............000000000000...
```

`offset_sides` asked `normal_at(path, i - 1, 0)` for the normal of the segment arriving at vertex
`i`, but `normal_at(_, index, 0)` *already* means "the segment ending at `index`", so the call was
one segment too early. At a two-point path's last vertex both lookups miss and the normal fell
back to a fixed `(0, 1)` — right for a segment running in the positive x direction, wrong for
one running in the negative x direction, which is the whole of the l2r/r2l split above. It
predated the pattern fix and reproduced with a plain `0 0 0 RG` stroke. **It is now fixed**, in
its own commit and with its own corpus measurement, as D13; the pattern fix above is D12.

Fixing it turned up three more places the same wrong argument had reached — the joins, both
caps, and the seam of a ring — and one much larger defect standing next to them, which is written
up as **D14**: an open path of three or more points is stroked as a **closed ring**, so it carries
a stroke along its own closing segment and has no caps at all. `mutool` draws an `L` as an `L`;
this drew the `L` plus a diagonal. That one is the next thing on the stroke item.

Two smaller things the same page exposed, both left open: **`Do` of a `/Subtype /Form` XObject
is not implemented** — `/Fm0 Do` became an image mark and reported `an image claims to be 0 by 0
pixels and was not drawn`, harmless here because the form is empty and wrong in general — and
**`/CS0` cannot be converted**, twice, because it is an `[/ICCBased]` space whose profile is not
read. The two fills it names are white, so neither costs anything on this page.

## Metric-compatible aliases

`metrics::widths` and `metrics::substitute` now answer for three families that are not on the
list of fourteen, because their metrics *are* a standard family's:

| alias | answers with |
|---|---|
| `Arial`, `ArialMT`, `Arial-BoldMT`, `Arial-ItalicMT`, `Arial,Bold`, `ABCDEF+Arial`, … | Helvetica |
| `TimesNewRoman`, `TimesNewRomanPSMT`, `TimesNewRomanPS-BoldMT`, … | Times |
| `CourierNew`, `CourierNewPSMT`, `CourierNew,Bold`, … | Courier |

`split_name` already stripped the subset prefix and the trailing `MT`/`PS`, so one entry per
family covers every spelling a producer writes. The alias is a **name→table mapping and nothing
else**: the tables are the same Adobe metrics, verified as before, and no number was changed.

**What this is not.** The refusal recorded in `metrics.rs` was not "non-standard names are
refused"; it was that answering a name by inferring a font turns a producer's guess into our
facts, which is why `Helv` still refuses. `Arial` is not a guess about a font — it is a font
whose advances are Helvetica's, which is the whole reason every metric-compatible clone of it
exists. So the decision stands; this is the other side of it.

**`HelveticaNeueLTStd-*` still refuses, deliberately and on the merits.** Helvetica Neue is not
metric-compatible with Helvetica — a different set of glyphs with different advances — so
mapping it would not be an approximation a reader could tolerate but a number they could
measure to be wrong. The corpus names a dozen spellings of it and every one refuses. The same
goes for a suffix that changes the face: `Arial-Black` is not `Helvetica-Bold` and refuses,
because `style_words` does not know the word `Black`.

**What was measured.** The real Arial, Times New Roman and Courier New are proprietary and are
not present here, so the pairings could not be checked against the fonts themselves. What can be
checked is the other side of the claim, and was, with `fontTools`: every glyph of the ASCII
range of all thirteen bundled Liberation faces against the table they stand in for,
`advance × 1000 / unitsPerEm`. **No ASCII glyph differs by more than one thousandth of an em in
any face** — most are exact, the rest are 2048-unit rounding. Two non-ASCII extras do differ
materially (`periodcentered` and `macron`), and they are outside the range prose lives in. As a
control, URW's own Adobe-metric clones match all 95 ASCII widths *exactly*, which is what
confirms the tables are Adobe's and that the Liberation ±1s are rounding rather than
disagreement.

**No corpus page moved, and here is why.** Of the eight corpus files that name one of these
families, four (`gov__nist-sp800-88`, `gov__nist-fips197`, `gov__nist-nistir7657`,
`gov__usgs-topo-cnmi-1`) **do not open at all** — "the page tree root is missing" or "no
catalogue" — so no before or after exists for them. Of the four that do open, none draws a
single glyph in an alias-named font on any page: `pdfjs__issue16263.pdf` declares
`TimesNewRomanPSMT` and `SymbolMT` but its page is two zero-by-zero images; `pdfbox__FC60_Times.pdf`
has one glyph, a space; `pdfjs__TAMReview.pdf` names the two Times aliases in its resources but
the only page carrying them (23) is two images with no text. Page 1 of each measured identical
before and after — `pdfjs__TAMReview.pdf` at SSIM 0.925334 and 129 851 ink against `mutool`'s
0.925334 and 147 677. So the change is asserted at the unit level instead:
`a_metric_compatible_alias_draws_exactly_as_the_family_it_stands_in_for` renders an unembedded
`Arial`, `Arial-BoldMT`, `TimesNewRomanPSMT`, `TimesNewRomanPS-ItalicMT` and `CourierNewPSMT`
page and asserts the pixels are **identical** to the page naming `Helvetica`, `Helvetica-Bold`,
`Times-Roman`, `Times-Italic` and `Courier`. That test fails on the previous code with "the
font `/F1` has no `/FontDescriptor`", which is the blank page the alias exists to remove.

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

**34 files did not open. 73 of the 77 open now.** 29 of them said `dangling reference: the
page tree root is missing`, 4 said `no catalogue`, and 1 was `the document is encrypted:
incorrect password`. That was one dominant bug, and it was not the one first written down.

The reader could not read a cross-reference *stream* at all. A cross-reference stream is an
indirect stream object, `N G obj << dict >> stream …`, and `Parser::next_object` parses
**direct** objects only — it stops at the `<<` and never reaches the `stream` keyword, so it
can never return a stream. `xref::read_section` asked it for one, which is unsatisfiable.
Underneath that was a second defect: the test for whether `/Index` was present asked
`get("Index").and_then(Object::as_i64)`, and `as_i64` of an array is always `None`, so
`/Index` was ignored in every case and an incremental update's seven entries were numbered
from zero. With no usable table the reader fell back to scanning for `N G obj` headers, a
scan cannot see inside an object stream, `/Pages` was unreachable, and the error named the
page tree while the fault was two layers above it.

The first account of this said the PNG predictor in `/DecodeParms` was not decoded. The
correlation was exact — every xref stream with a predictor failed, the one without opened —
and it was wrong: most PDF 1.5+ producers emit Flate with `/Predictor 12`, so the
correlation described who writes these files, not what broke them. Three experiments
exonerated the predictor before anyone asked what `next_object` actually returns for
`N G obj <<…>> stream`. D2 in `docs/known-diffs.md` keeps the correlation table and the
disproved theory, because a plausible story that survives three experiments is worth being
able to point at.

| | before | after |
|---|---|---|
| files opened | 43 | **73** |
| files not opened | 34 | **4** |

Every file this bug broke now reports **`as written`** — the reader uses the file's own
cross-reference information and repairs nothing — and page counts match `pdfinfo` on all 73.
That includes live government forms (IRS 1040 and W-4, NIST FIPS 197 and SP 800-88), two
1:25000 USGS topo sheets and every Adobe InDesign output in the set. The four that still
fail are two fuzzed files `qpdf` also cannot read, one correctly-reported encrypted file,
and one encrypted file this reader misdiagnoses; all four are itemised in
`docs/known-diffs.md`.

Two smaller defects in the same code were ways of *inventing* structure, which is the one
thing this reader must never do: a cross-reference stream that decoded to a third of what
`/Index` promised was believed anyway, and an object number too large for a `u32` was
silently renumbered onto object 0, overwriting the free-list head. Both now refuse the
section and let recovery rebuild, reporting the damage.

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
CFF table, where a character code reaches a glyph through the CFF charset, and nothing read
the charset. **That half is now fixed** — see D8 — and the same arXiv page is the measurement
of it. Another 67 pages name a base-14 font that is not embedded at all; **that half is now
fixed as well**, with the metrics above.

**The arXiv page's SSIM went from 0.791534 to 0.878445 when the charset was fixed, and from
there to 0.88414 with the substitute faces; its ink went from 0 to 61 262 at the charset fix
and higher still now.** `mutool` puts 130 395 pixels on that page, so a good part of it is
still missing — the images, and text in faces that are neither standard fourteen nor
substitutable. `pdfjs__freeculture.pdf` p4 goes from
0.947615 and no ink at all to 0.992561 and 24 954 against `mutool`'s 25 929 — a page that was
blank because of the charset, measured against the oracle, four per cent of the ink away.

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

### Five defects in one place, found by measuring the fax pages against an oracle

`pdfbox__multitiff.pdf` page 1 is a page-sized CCITT G.4 scan. Against `mutool draw` at
150 DPI it scored 0.38414 SSIM with 59% of the page's ink against the oracle's 8%, and it is
the clearest single illustration in this project of a fixture-shaped blind spot: **the
project's own fax fixtures all end on a white run**, and a defect that only shows on a row
ending in black is invisible to every one of them.

Four defects, in the order they had to be found, and a fifth that only became visible once
the fourth was fixed:

1. **A fax decoder's output was read as a packed bit stream.** `ccitt_decode` returns one
   byte per pixel, and reading that as `/BitsPerComponent` claims — one bit — takes eight
   pixels out of every byte and smears each row sideways by a factor of eight.
   `mangle_syntax::stream::Decoded` now carries `one_byte_per_sample`, set by the decoder
   rather than guessed by a consumer from the filter's name, because only the decoder knows
   what it produced.

2. **Then the same number was read as a value range, and the page went black.** Fixing the
   first by passing `bits = 8` was right about layout and wrong about everything else:
   `to_rgba` divides a sample by `2^bits − 1`, so the byte 1 — which is *white* in a fax
   scan, and most of one — became 1/255, which is black. **Layout and value are two
   different facts, and only one of them comes from the decoder.** Layout is how many bits of
   each byte a sample occupies, and the decoder says that; value is what a sample is worth,
   and `/BitsPerComponent` says that, because it is the file's statement of the range its
   samples span. `image::decode` now carries the two apart in a `SampleRange`.

   | | SSIM | RMS | above tol | ink ours / oracle |
   |---|---|---|---|---|
   | before (one number for both) | 0.38414 | 197.78 | 60.59% | 59.0% / 8.0% |
   | layout from the decoder, value from `/BitsPerComponent` | 0.91170 | 72.09 | 7.99% | 0.0% / 8.0% |
   | and the T.6 codec reading the coding line's coordinates | 0.86317 | 88.84 | 12.14% | 7.98% / 8.0% |
   | **and the image placed where its matrix says** | **0.99747** | **4.72** | **0.03%** | **7.99% / 8.0%** |

   The third row scores *lower* than the second and the page is much closer to right. 0.91170
   was the score for drawing nothing at all, and a page with no ink on it agrees very well with
   a page that is 92% paper. The third row draws the whole scan — the oracle's amount of ink, in
   the oracle's horizontal band — and puts it in the wrong rows, because `image::draw` rebased
   its writes to the origin of the clipped area and mirrored its samples about the horizontal
   axis. That was a placement defect in the image path; it predated every defect on this page,
   and it is now fixed with its evidence as D5b in `docs/known-diffs.md`.

3. **An `/ImageMask` painted the wrong bits, and with a colour nobody asked for.** `Do` names
   no colour, so a mask's colour comes from the graphics state and has to travel with the
   mark; the painter had been handed black unconditionally. Separately, the stencil test
   compared a *normalised* sample against one half, so a CCITT mask's every sample read as
   ≤ 1/255 and nothing distinguished a bit that paints from one that does not. A stencil's
   sample is a bit however wide a byte the codec wrote it into, so it is now tested on the
   stored bit. A pattern colour is *evaluated* rather than guessed at: a shading pattern is now
  painted per pixel, which is the third thing this row once refused and now draws.

4. **The CCITT decoder dropped a line's final run.** `Line::samples` fills up to each change
   point and stops, but a change point says where a run *ends*, so everything after the last
   one kept the row's opening colour: `white 3, black 5` decoded as eight white pixels. Every
   fax fixture here ends on a white run, which is the one case where that looks right.

### The T.6 two-dimensional path, which was broken in four ways at once

Fixing 4 did **not** move the row-recovery figure, because the last-run defect and the
two-dimensional defect were independent. What is below is the whole of it, and the claim that
used to sit here — that the two-dimensional path was still broken — was wrong: it was broken in
four ways, every one of them invisible to a specification-derived test, and all four are fixed.

Decoding `pdfbox__multitiff.pdf` page 1's image now recovers **287 of 287 rows, byte for byte
against libtiff, with 0 damaged rows and nothing truncated**, at `DamagedRowsBeforeError` of
both 0 and 1. Against the whole of the frozen ground truth, **all 15 libtiff cases match byte
for byte** — 896 of 896 rows. Before the fix the same fixture gave 15 of 287.

**Four defects were in `decode_line_2d`.**

1. **A run length was added to the reference line's position.** `a0` is the *coding* line's
   current changing element; `b1` and `b2` are positions on the *reference* line. The two run
   lengths a horizontal-mode pair carries are counted from `a0` in the coding line, and they
   were being added to `b1`. Rows 0–3 of this image decode correctly as all white; row 4 is a
   horizontal-mode pair, so `b1` is the line width, 344, the black run lands outside the row,
   no change point is recorded, the line comes out blank, and every later line is read against
   a reference that was never written until the bit reader meets a sequence that is not a mode
   code, at bit 24 of 1768. The runs really are the ones the file encodes — 199 white then
   9 black, against `libtiff`'s row 4 of white to 199, black for 9, white after. This one
   defect is 272 of the page's 287 rows. The pair also recorded only its *last* changing
   element, so the first run's colour was lost with it.
2. **`b1_b2` read the reference line's colour at `a0`.** `b1` is the reference line's first
   changing element to the right of `a0` whose colour differs from the **coding** line's colour
   there, and the two lines are coded against each other and may disagree there. The search
   compared against the reference line's own colour at that position, so it returned an element
   that is the coding line's *own* colour, which is never the useful one; it also located `b1`
   by walking pixels for the first place the two lines differ, which answers `a0 + 1` whenever
   the coding element sits inside a reference run of its own colour.
3. **The elements' polarity was inverted.** An element was called black when its index in the
   change list was odd. A line opens white, so the *first* change is the black one: every
   element was labelled with the colour it changes away from.
4. **Three of the eight `MODE_CODES` entries were mistranscribed and a ninth was missing.**
   `0001` was read as `Vertical(-2)` and is *Pass*; `000011` was read as `Vertical(2)` and is
   *Horizontal*; `0000011` was read as `Vertical(-3)` and is *Vertical(+3)*; `0000001` was read
   as *Pass* and is not a T.4 code at all, while `0000010` — *Vertical(-3)* — was missing, as
   was *Vertical(+3)*. T.4 table 4 has nine mode codes; the table had eight.

The table is now read off libtiff's encoder rather than from memory, which is the only reason
to trust it: a reference line holding one black run and the same line shifted by `d` can only
be a pair of vertical modes, and the bits libtiff writes for `d = -3` are `0000010`. A shifted
version of a correct table survives review, and no assertion derived from the specification
can tell one from the other.

**What makes a rule this size testable is an expectation another implementation wrote.**
`scripts/make-ccitt-fixtures.py` has `tiffcp -c g4` encode a raster and `tiffcp -c none` decode
the identical bytes back, and freezes the stream and the raster together under
`crates/mangle-filters/tests/fixtures/ccitt-g4/`; `crates/mangle-filters/tests/ccitt_libtiff.rs`
reads the pair and asks whether our decoder produces the second from the first. The fixtures
are checked in, so the test runs on a machine with no libtiff at all — a test that only runs
where the oracle is installed is not a test.

Group 3 2D is the one variant libtiff is **not** an oracle for: its decoder rejects every
first line built to T.4 with `Line length mismatch at line 0`, including an all-white one,
and its encoder writes that line as a vertical zero where T.4 requires the horizontal pair.
Its encoder and decoder agree with each other and with neither T.4 nor anything else, so
freezing a fixture from them would freeze the disagreement. That path is still checked against
hand-written bit strings in `src/ccitt.rs`, with every code named in the comment above it.

### The placement, which put a decoded scan in the wrong rows

The codec being right is not the image being right, and the page that proved it is the one
where the two differ: the whole page is the scan, so if the placement is right then the two
renderings agree to within antialiasing, and if it is wrong then a decoded scan is still in the
wrong rows. `image::draw` had two independent errors, either of which alone puts an image in
the wrong place:

1. **The write was rebased and the bounds were not.** `bounds` comes from `matrix.apply` in
   absolute device pixels and `area.pixels()` keeps it there, so the loop walked absolute
   coordinates and then wrote at `x - x0, y - y0` — where `x0`/`y0` are the *clipped area's*
   origin. Every image not at the page origin was drawn at the page origin. `x0`/`y0` are
   deleted rather than left at zero: a binding named for an origin that is no longer subtracted
   is the same mistake one refactor away.
2. **The vertical axis was mirrored.** `placement.matrix` inverts y, so the unit square's
   `v = 0` is the placement's *bottom*, while `Raster::sample` reads `v = 0` as raster row 0 —
   the scan's *top*. Every image was upside down. The flip now goes into the **mapping**:

   ```rust
   let to_image = matrix.concat(Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0));
   ```

   and not into the raster's buffer. That is the part a fix by inspection gets wrong: a flipped
   buffer gives identical pixels for every image drawn square to the page — all of the corpus's
   and all of the old fixtures — and different ones the moment the image is turned, because the
   turn belongs to the placement and a flipped buffer is not carried by one. `mutool draw`
   settles it: `0 200 -200 0 200 0 cm` over a four-coloured quadranted image puts the image's
   top-left in the page's **lower** left, which is where a counter-clockwise quarter turn takes
   that corner; the buffer-flip version puts it in the upper left.

**What the two compose into, and why the SSIM could not see it.** A rebase and a mirror are a
*mirror about the middle of the placement rectangle*, not a shift, so the best pure vertical
translation explained only **0.6270** of the oracle's ink at 650 rows. That figure is now
**0.9972 at no shift at all**, and the two intermediates are what make the check worth having:

| | best shift | translation explains | SSIM |
|---|---|---|---|
| both defects | 650 rows up | 0.6270 | 0.86317 |
| mirror fixed, rebase not | 719 rows up | 0.9972 | — |
| rebase fixed, mirror not | 650 rows up | 0.6270 | — |
| **both fixed** | **none** | **0.9972** | **0.99747** |

**A fraction cannot see a rebase** — with the mirror fixed and the rebase not, the translation
explains 0.9972 of the oracle's ink while the image sits 719 rows in the wrong place, because a
shift absorbs a constant displacement exactly. **A shift cannot see a mirror**, because a mirror
is not a shift. So the test asserts the figure *and* the shift. SSIM says neither: 0.91170 was
this page's score for drawing nothing at all, because a blank page agrees with a page that is
92% paper.

**Why nothing caught either.** The five image tests in `crates/mangle-render/tests/pages.rs`
drew a 2 × 2 of pure red and `0b0000_1111` repeated on all eight of its rows — neither has
vertical structure, so a mirror is invisible — through `cm`s whose translation is 0, so the
clipped area's origin is the page's own and the rebase subtracts nothing. Every assertion was on
columns. What is there now, each confirmed to fail on the code before it: a banded image at a
non-zero `cm` translate, asserted pixel by pixel at three scales; the same image at two
translations, each checked to leave the other's rows bare; a quarter-turned four-quadrant image
asserting the *oracle's* orientation, which is the test a buffer-flip fix fails and a mapping
fix passes; a full-page image at the origin, which must not regress; and the corpus page
itself, where every candidate vertical shift is tried over per-row bitmaps and both the best
one and the fraction it explains are asserted. Three more of the same shape are unit tests in
`image.rs` over a two-by-two raster of four colours, so the placement is pinned at the pixel and
not only through a page.
