# Status

Where the work actually is. Updated whenever a milestone moves.

## Milestones

| M | Deliverable | State |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen, window shell, icon pipeline | **done** — every Gate 0 check passes; the window draws the six regions from tokens and nothing else |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector | **mostly done** — everything except the Inspector. See "Gaps" below |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell | **partly done** — the tokeniser, the operator table, the graphics state and the interpreter exist, and the rasterizer now turns a page's paths into pixels with analytic coverage. Text, images, shadings and patterns still draw nothing |
| **M3** | All fonts, colour spaces, patterns, shadings, transparency, JBIG2/JPX, OCGs | not started |
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
- **`mangle-cli`** — the headless surface: `info`, `pages`, `check`, `extract`, `save`.
  This is how "what does ManglePDF think of this file?" is asked without a window.
- **`mangle-ui`** — the window shell and the design tokens. Six regions, one grid, one
  stroke weight, a light and a dark theme, and a contrast test that both themes must
  pass.
- **122 hand-authored SVG icons**, lint-clean, with a contact sheet.
- **Docs** — architecture, status, development, testing, PDF quirks, dependencies,
  icons, and the decision records.

## Tests

405, none ignored, no warnings. Five kinds matter:

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
- **Rendering** — a page is rendered and checked against what the page says should be on
  it: each shape where the file put it, the clip where the file put it, the ink matching
  the shapes' areas to the luminance. The corpus is rendered at two scales to prove no
  file hangs or overruns the buffers.
- **The rasterizer against `mutool`** — the same pages rendered by `mutool` and compared.
  `mutool` is an independent implementation with years of accumulated knowledge of what a
  page should look like, so agreeing with it is the only check that says anything about
  correctness rather than about internal consistency. The comparison is on *ink*, with both
  renderings flattened onto one background first: `mutool` writes an unpainted page as
  transparent and this renderer writes white paper, and comparing the raw buffers would be
  comparing conventions. Skipped cleanly when `mutool` is absent.

## Gaps, in the order they block

1. **Text draws nothing.** A glyph run is reported with its font, its bytes and a
   placement per byte, but no glyph is painted: deciding which bytes of a string are
   glyphs and how wide each is needs the font and the encoding, and the content layer
   deliberately refuses to guess. Text is the next substantial piece.
2. **Images, shadings and patterns draw nothing.** Each is reported as a note against the
   mark rather than skipped silently, so a page that used one is visibly incomplete
   instead of quietly wrong.
3. **A clip is honoured as its bounding box.** A clip path can be any shape; what the
   interpreter records is the rectangle that bounds it, which draws slightly more than it
   should rather than slightly less. The renderer needs the real clip region.
4. **The Inspector does not exist.** `mangle-ui` draws the region; nothing populates it
   from the marks the content layer produces.
5. **JBIG2 and JPX are not implemented.**
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
