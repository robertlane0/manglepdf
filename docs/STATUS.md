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
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | **partly done** — the tokeniser, the operator table, the graphics state and the interpreter exist, and the rasterizer now turns a page's paths and its embedded-TrueType glyphs into pixels with analytic coverage. Images and shadings draw; patterns and the non-TrueType outlines still draw nothing |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | **partly done** — the four PDF function kinds, axial and radial shadings and the device colour spaces paint. Mesh shadings, tiling patterns and transparency do not |
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

577, none ignored, no warnings. Ten kinds matter:

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
- **Widths against two other renderers** — the standard fonts' metrics are checked by
  measuring where a real renderer puts each glyph, not by reading the table back. One
  transcription error was found this way: `fraction` in Helvetica had the width of the URW
  clone, 278, where Adobe's own metrics say 167, and poppler agrees with Adobe. Every width
  in every table was then re-checked against the metrics files, and all 1043 agree.

## Gaps, in the order they block

1. **A clip is only in force for the mark that follows it.** The interpreter records the clip
   on every mark, but the renderer applies it when it reaches a `W n` and drops it after the
   next mark is drawn, because the mark's own cull and that reset are the same rectangle. A
   page that clips once and then draws twenty things draws nineteen of them unclipped. The
   fix is for the render loop to trust each record's clip rather than the mark before it,
   which means deciding whether re-applying a clip is cheap enough to do per mark or wants a
   handle comparing clips by identity.
2. **Only embedded TrueType outlines draw.** A glyph is filled from a `/FontFile2` program
   and compared against `mutool` at 0.962 SSIM. Four kinds are still missing: the standard
   fourteen, which have no program at all; CFF and Type 1, which need a charstring
   interpreter rather than a table walk; Type 3, whose glyphs are content streams; and
   composite fonts (Type 0 and Identity-H), whose character codes are two bytes rather than
   one. A page using one is reported rather than drawn blank.
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
- A page's pixel buffer is `ceil(points × scale)` on each side, and a product that lands a
  hair over a whole number in binary gives one pixel more than the comparison oracles give.
  It only shows on a page whose width is a whole number of pixels at the requested
  resolution, and the oracle comparison refuses rather than comparing different sizes.
