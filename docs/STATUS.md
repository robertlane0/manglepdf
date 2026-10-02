# Status

Where the work actually is. Updated whenever a milestone moves.

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
  with a note rather than failing.
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

534, none ignored, no warnings. Eight kinds matter:

- **Unit** — one behaviour, stated expectations, including a documented quirk for each.
- **Round trip** — open a file, change it, write it, open it again, compare. This is
  what makes the lossless claim testable rather than asserted.
- **Oracle** — `qpdf` reads what we write. It caught a wrong `/Size`, a cross-reference
  that padded sparse numbering, and a fixture that was wrong rather than damaged. When
  `qpdf` is absent these tests skip and say so; `cargo test` never needs it.
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
- **Fidelity against `mutool`, measured** — the shapes page and the clipped page rendered
  by `mutool` at 150 DPI (the resolution the acceptance criteria name) and compared with
  SSIM. Shapes **0.99829**, clipped **0.99917**, against a bar of 0.95. Both renderings are
  flattened onto one background first, because `mutool` writes an unpainted page as
  transparent and this renderer writes white paper, and comparing raw buffers would compare
  conventions rather than renderers. Skipped cleanly when `mutool` is absent.
- **Widths against two other renderers** — the standard fonts' metrics are checked by
  measuring where a real renderer puts each glyph, not by reading the table back. One
  transcription error was found this way: `fraction` in Helvetica had the width of the URW
  clone, 278, where Adobe's own metrics say 167, and poppler agrees with Adobe. Every width
  in every table was then re-checked against the metrics files, and all 1043 agree.

## Gaps, in the order they block

1. **A clip is honoured as its bounding box.** A clip path can be any shape; what the
   interpreter records is the rectangle that bounds it, which draws slightly more than it
   should rather than slightly less. A diagonal clip and a circular one are both boxes now.
   The renderer needs the real clip region, and the renderer is where it belongs: the
   interpreter's job is to say *what* was clipped, not to rasterise it.
2. **CFF and Type 1 glyphs do not draw.** Embedded TrueType does: `mangle-font` reads the
   `/FontFile2`, walks the glyph — composites come out whole, because `ttf-parser` follows
   the component list — and the renderer fills the outline through the same filler a path
   uses, with the glyph's own text rendering matrix. A page of text in an embedded
   TrueType font scores 0.962 against `mutool` at 150 DPI, so this is compared rather than
   assumed. What remains: the outlines of a `/FontFile3` (CFF, and OpenType with `CFF `),
   the standard fourteen, and Type 0 with Identity-H, whose two-byte codes the interpreter
   does not yet split.
3. **JBIG2 and JPEG 2000 have no decoder.** An image needing one is reported by name rather
   than drawn as a blank rectangle, because a page with a conspicuous hole is a bug report
   and a page with a missing photograph is a wrong answer. CCITT does decode, through the
   filter crate.
4. **Tiling patterns and mesh shadings draw nothing.** A shading names one of types 1, 4, 5,
   6 or 7, or a pattern needs a tiling loop, and each is reported as a note against the mark
   rather than skipped silently, so a page that used one is visibly incomplete instead of
   quietly wrong. Images and axial/radial shadings now draw.
5. **The Inspector does not exist.** `mangle-ui` draws the region; nothing populates it
   from the marks the content layer produces.
6. **An object that came out of an object stream cannot keep its original bytes**,
   because it had none: it was compressed with everything else in its container. A full
   save writes it as a direct object, which every reader accepts but which is a
   re-serialisation rather than a copy.
7. **No signature writing.** `ByteRange`, CMS and DocMDP all still have to be built.

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
- `mangle-filters::deflate` emits a raw deflate stream (RFC 1951), not the zlib wrapper
  (RFC 1950) that PDF's `/FlateDecode` names. Our own `inflate` tolerates the missing
  wrapper, so nothing here has noticed, but a `/FlateDecode` stream we *write* is not
  readable by anything else. Every filter we produce needs checking against a real reader
  before the writer ships.
- A page's pixel buffer is `ceil(points × scale)` on each side, and a product that lands a
  hair over a whole number in binary gives one pixel more than the comparison oracles give.
  It only shows on a page whose width is a whole number of pixels at the requested
  resolution, and the oracle comparison refuses rather than comparing different sizes.
