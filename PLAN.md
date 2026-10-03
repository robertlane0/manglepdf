# ManglePDF — Plan

> The authoritative specification is [`GOAL.md`](GOAL.md); the definition of done is
> [`FINISH.md`](FINISH.md). This file is the *working* plan: what is being built right
> now, in what order, and why. Keep it short. Living status lives in
> [`docs/STATUS.md`](docs/STATUS.md); architectural decisions live in `docs/decisions/`.

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
| ADR-0003 | In-house Flate **inflate** (must return partial output for truncated streams) and in-house deflate | `flate2`/`miniz_oxide` abort on corruption; a bomb must return partial data, not fail |
| ADR-0004 | egui + eframe (winit) for the GUI, everything custom-painted from design tokens | Pure Rust, no toolkit look to fight, easy canvas, HiDPI, IME, and accessibility via AccessKit |
| ADR-0005 | `ttf-parser` + `rustybuzz` for shaping only; all PDF font program handling (CFF, Type 1, subsetting, CMaps, encodings) in-house | Shaping is not PDF-shaped; font *program* handling is (Charter §2.3) |
| ADR-0006 | Permitted codecs only for JPEG (DCT) decode/encode; PNG is in-house on top of our own Flate | PNG needs exact control of predictors/bit depths; JPEG is too large to justify rewriting twice |
| ADR-0007 | Own analytic-coverage scanline rasterizer shared by PDF rendering *and* SVG icon rendering | One rasterizer to get right; dogfoods the renderer (Charter §8.7) |

## Composite fonts (settled)

A `/Type0` font's codes are two bytes, so the byte count and the glyph count stop agreeing.
Four things changed together, and each is a place where a wrong answer is invisible rather
than loud:

- **The code width is the font's claim, not an inference.** `Resources` records which
  resource names are composite (`/Subtype /Type0`), and `Tf` puts that on the text state
  beside the widths. Inferring it from the presence of a widths array would be wrong in both
  directions: a composite font may declare no `/W` at all, and a simple font always has one.
- **The string is split into codes, and everything downstream counts codes** — placements,
  advances and `TJ` kerns. A trailing odd byte is dropped rather than padded: half a code
  names no glyph, and a glyph nobody asked for is worse than a character missing from a
  truncated string.
- **The width comes from the run-length `/W`.** `CidWidths` keeps the runs as written
  (`c [w1 w2 …]`, `c_first c_last w`) rather than expanding them, because a range run can
  cover the whole 16-bit code space and expanding it would cost 65 536 entries to answer a
  subtraction. `/DW` is 1000 for a code no run covers — the specification's answer, not a
  fallback invented here. `Declared` is untouched: a Type 1 font is the common case and must
  not pay for a two-byte path.
- **The mark carries the codes.** `Mark::Glyphs` has `codes: Vec<u32>` and `two_byte: bool`
  beside the raw `text`. A mark that said only how many bytes it had would leave every
  renderer to guess, and guessing one byte per code is the bug. `/ToUnicode` is not read —
  it is text extraction's, not the drawer's — but the mark now carries what it will need.

## CFF charstrings (settled)

A `CFF ` table's glyphs are Type 2 programs, not point lists, so reading one means running
it: a stack machine over `f32`, local and global subroutines, the subroutine-number bias,
stem hints, and a width that is either the first operand on the stack or absent. Two
decisions are worth recording because the wrong version of either produces a *plausible*
wrong glyph rather than an error:

- **A curve operator's three points are offsets from the point before each of them**, the
  first from the pen, the second from the first, the third from the second. Reading all
  three from the pen yields a closed, well-formed glyph that is simply too narrow. Every
  glyph of every CFF font on this machine is now cross-checked against `ttf-parser`'s
  independent interpreter, coordinate for coordinate, because one hand-written fixture
  cannot see this class of mistake.
- **Three cases are refused with a reason rather than drawn blank or drawn wrongly.** A bare
  `/Subtype /Type1C` name-keyed font cannot resolve a character code, because that needs the
  charset plus the `/Encoding` — its outlines are readable, so the report says exactly that.
  A CID-keyed CFF is read through `/FDArray` and all three `/FDSelect` forms, but only under
  an identity charset, since a renumbering one would silently substitute letters. `seac` and
  a Top DICT declaring `CharstringType 1` both build one glyph from two others *by name* and
  are refused for the same reason.

## Type 1 charstrings (settled)

A Type 1 font is a PostScript program that happens to be stored in two encrypted halves, so
reading a glyph means three things in order: find `eexec`, undo the outer cipher, undo a
second inner cipher over the charstrings. Each layer is `p = c XOR (r >> 8)` with
`r = ((c + r) * 52845 + 22719) mod 65536` — and the recurrence is fed the **ciphertext**
byte, which is the one place the algorithm can be read two ways and the wrong reading still
produces printable output. The seeds are what separate the layers: 55665 for `eexec`, whose
first four plaintext bytes are a salt to discard, and 4330 for the charstrings, advanced
`lenIV - 4` times, which is why the usual `lenIV` of 4 — and a font that mentions none —
needs no advance at all.

Five things are worth recording, because the wrong version of any of them produces a
*plausible* wrong glyph rather than an error:

- **`hvcurveto` and `vhcurveto` take four operands whose fourth is the endpoint's x for
  one and its y for the other** (`dy1 dx2 dy2 dx3` and `dx1 dx2 dy2 dy3`). Reading the fourth
  and fifth the wrong way round swaps them, which turns every round letter into a shape that
  closes and does not match. Cost 0.15 of SSIM on a page of ordinary text before it was found.
- **`hsbw`'s first operand says where the outline sits relative to the pen**, and the
  charstring's coordinates do not include it. Leaving it out puts every glyph a whole
  sidebearing to the left of where it belongs.
- **There is no subroutine-number bias.** Adding CFF's lands on a different, in-range
  subroutine and runs the wrong program.
- **A hint mask's length is a property of the code it is written in**: a subroutine's mask
  covers the stems *that subroutine* declared, so it is empty unless it declares stems of its
  own. Reading it from the caller's running total skips bytes that belong to the subroutine's
  operators, which turns the standard hint-replacement subroutine into a prefix of nonsense.
- **The absolute-`dy` convention is not applied, and that is measured rather than assumed.**
  The specification records that the last `dy` of the first `rrcurveto` after a mover is an
  absolute y. Every one of the 28 Type 1 faces installed here was converted from a CFF
  outline and is written to the *other* convention, and applying the rule moves the endpoint
  of the first curve in 18 of the 94 printable ASCII glyphs of `NimbusSans-Regular`. The same
  face is installed as OpenType/CFF and can be compared glyph by glyph: 18 mismatches with the
  convention applied, 2 without, and `mutool` — the oracle the acceptance criteria name —
  agrees with the reading without it.

**Refused, each with a reason rather than a wrong shape:** a PostScript `OtherSubrs`
procedure with no conventional number (the four that have one — the flex at 0, 1 and 2, and
hint replacement at 3 — are implemented); Multiple Master `callothersubr` 14–28; `seac`;
operator 15, which is reserved and undocumented; and a PFB container, whose segment headers
are not stripped. **Hint replacement is refused and nothing else is**: it asks for the stem
hints to be recomputed for a rasterizer's pixel grid, this renderer computes exact analytic
coverage and has no grid, so the hint is dropped and the glyph's geometry is untouched.

A page's own `/Encoding` is *not* consulted inside the Type 1 reader — the font's own is, which
is what makes a Type 1 program able to resolve a character code at all where a bare CFF cannot.
The page's encoding is consulted one layer up instead, by `mangle-render`, which turns the
code into a glyph *name* before asking any font program for a glyph (see "Encodings"). Putting
it here would have made this the only font type whose page encoding was honoured, and putting it
in the interpreter would have made it so for no font type at all.

## Encodings (settled)

A character code means nothing on its own. It is the `/Encoding` that turns it into a glyph
*name*, and the name that a font is looked up by — which is why a page that remaps one code
through `/Differences` used to render that code with the wrong glyph for every font type. The
tables are in `mangle-font::encoding`, transcribed from Table D and Annex D.2, and each was
cross-checked against a source already on the machine rather than trusted: Ghostscript's own
decoding table for StandardEncoding, its init file's construction for WinAnsi, and the
machine's `mac_roman` code page together with poppler's `MacRomanEncoding` for MacRoman, the
last two agreeing on every code from 32 to 225. The AGL resolver holds 912 names from Adobe's
Glyph List, which is a machine source — so the Greek and Cyrillic blocks are complete rather
than restricted to Latin.

Three decisions worth recording:

- **The `/Encoding` is read in `mangle-render`, not in the font readers and not in the
  interpreter.** `mangle-font` is below the content layer and cannot see a page; the
  interpreter would have to know about fonts to ask this question. One call site, one place
  the answer can be wrong.
- **A composite font keeps the CID path untouched.** Its code is a CID, not a character code,
  and the specification says `/Differences` does not apply to one. Applying an encoding to a
  number that is not a character would be wrong in the same way a CID looked up through a
  `cmap` would be.
- **A name reaches a glyph by two routes, and a Type 1 font by the direct one.** A Type 1
  program's `CharStrings` are keyed by name, so the name *is* the lookup. A TrueType font is
  found through the `post` table if it carries names and otherwise through the Unicode
  subtables by way of the AGL — the (3,0) subtable stays reserved for the code, because that
  is a symbolic font's author's own choice of code for a glyph and it is already consulted in
  the right order.

Verified at **0.98780 SSIM** against `mutool` on a page remapping eighteen codes, four of them
in the Latin-1 range where WinAnsi and MacRoman disagree completely. The fixture's remaps are
not a permutation of each other and the page is asymmetric, so it catches ignoring the array,
stopping at the end of the base, and transposing two codes; renaming `/Differences` in the
file and re-scoring drops it to 0.895, which is how the test is known to see what it is for.

## Standard-14 substitution (settled)

The standard fourteen are defined by their *metrics* rather than by any one program, so a
document that names one without embedding it — which is most of the LaTeX and office output
in the wild, because those producers assume the reader has the font — is naming a font, not
naming nothing. Refusing it leaves a hole in the page where the text was. Twelve of the
fourteen now draw from bundled metric-compatible faces: Liberation Sans for Helvetica, Serif
for Times, Mono for Courier, unmodified and under the OFL.

- **It was two fixes, and the width one was larger.** The outlines are obvious — look the name
  up in a table and fill from the face. But `Resources::from_dict` only recorded a width run
  when `/Widths` was present, and the standard fourteen have none by definition, so an
  unembedded standard-14 font advanced *every* glyph by the 500-unit default. Drawing the
  glyphs without fixing the widths would have put every character after the first in the
  wrong column, which scores worse against the oracle than not drawing at all. The run is the
  built-in table indexed by **code**, read through the font's own `/Encoding` — a `/WinAnsi`
  font, a `/MacRoman` one and one with a `/Differences` array each get their own answers,
  because the width of 0x92 is a question about the document and not about Helvetica.
- **One name parser, not two.** `mangle_font::substitute` calls the same `split_name` and
  `style_words` the width tables use, so a subset prefix (`ABCDEF+Helvetica-Bold`) and the
  older comma spelling (`Helvetica-Bold,Italic`) resolve for the outline and the width
  together and cannot disagree with each other.
- **The substitution is reported, never silent.** The page carries a note naming the face
  that stands in, because the outlines on the paper are that face's and not the original's,
  and that is the one fact about the picture a user cannot read off it.
- **A composite font gets no stand-in.** Its codes are CIDs the document chose itself, so a
  simple face is not a substitute however well the widths happen to agree.
- **A family that stands in for a standard one by measurement is answered as that family.**
  `Arial`, `TimesNewRoman` and `CourierNew` are not on the list of fourteen and are the names
  a good deal of the wild corpus actually writes, so a table maps each onto the family whose
  metrics it shares. This is a name→table mapping and nothing more — the numbers are still the
  verified Adobe ones — and `split_name` has already stripped the subset prefix and the
  trailing `MT`/`PS`, so one entry per family covers `TimesNewRomanPSMT`,
  `TimesNewRomanPS-BoldItalicMT`, `Arial,Bold` and the rest without a second parser. **What
  this does not do is guess:** refusing `Helv` is refusing to infer a font from a producer's
  abbreviation, and that still happens. What is asserted instead is a property of a named
  font. `HelveticaNeueLTStd-*` is therefore still refused and is meant to be — Helvetica Neue
  is *not* metric-compatible with Helvetica, so mapping it would not be an approximation to be
  tolerated but a number a reader could measure to be wrong. A style word the family does not
  know refuses too, so `Arial-Black` is not quietly answered with `Helvetica-Bold`.

Five things this is *not*, recorded so nobody reads a higher SSIM as more than it is:

1. **`fi` and `fl` are absent from Liberation Sans.** They would have to be composed from
   `f`+`i` and `f`+`l` condensed to fit 500 units, against an `f` of 278 — an invented
   ligature rather than the font's own glyph, and the kind of plausible wrong shape this
   project refuses everywhere else.
2. **`macron` is the wrong macron.** The U+00AF glyph in Liberation is the *spacing* bar, not
   the AFM accent that sits over a base character. A name that resolves to a spacing bar
   where the metrics files say accent is a silently wrong glyph.
3. **`periodcentered` is drawn wider than Adobe's**, so a bullet or a middot in a list comes
   out fatter than the oracle's. The width is right, which is what keeps the line where the
   producer put it.
4. **`Symbol` and `ZapfDingbats` have no substitute and stay refused.** Liberation carries no
   symbol or dingbat face, and inventing a stand-in for a symbol font would put the wrong
   glyphs on the page. They are reported by name.
5. **This does not close every blank page in the corpus.** Measured against `mutool`,
   `pdfbox__data-000001` goes **0.93127 → 0.94703** with ink **448 654 → 477 471** against an
   oracle of 483 043, and `gov__arxiv-1206.5537` **0.87844 → 0.88414**. Both moved, neither is
   close to 0.99, and the rest of the gap is not only fonts. The alias table closed the naming
   half of the remainder but **no corpus page number moved because of it**, and that is a
   finding rather than a null: of the eight corpus files naming `Arial*`, `TimesNewRoman*` or
   `CourierNew*`, four **do not open at all** (`gov__nist-sp800-88`, `gov__nist-fips197`,
   `gov__nist-nistir7657`, `gov__usgs-topo-cnmi-1` — "the page tree root is missing" or "no
   catalogue"), and of the four that do open none draws a glyph in one on any page. So the
   alias is worth having and is worth **waiting to re-measure on** — it should matter a great
   deal to the two NIST reports, which are the corpus's densest users of `TimesNewRomanPSMT`
   (569 spans in `sp800-88`) and `CourierNew` (3069 in `fips197`) — but it cannot be, and is
   not, credited with a number it did not move.

**The cost is deliberate and on the record:** `include_bytes!` puts 4.4 MB of faces into
every binary that links `mangle-font`, with no way to leave them out. That is the price of a
renderer that never has to ask the machine for a font.

## Milestone map (detail in GOAL §10)

| M | Deliverable | Gate it unlocks |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen skeleton, themed empty window at 1536×1024, icon pipeline + gallery + lint | Gate 0 |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector object tree | M1 exit: F01–F11 |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell with thumbnails/zoom/nav | M2 exit: F31 p1 composition |
| **M3** | All fonts + CMaps, colour spaces, patterns, shadings, transparency, CCITT/JBIG2/JPX, OCGs | Gate 3 render fidelity |
| **M4** | Page objects + provenance, select/move/scale/delete/recolour/Arrange, undo/redo, first save→reopen-elsewhere loop | U1/U2/U3 |
| **M5** | Text model, editing, reflow, font resolution/embedding/`ToUnicode`, Text Properties panel | Gauntlet S1 |
| **M6** | AP generators, foreign annotations, comments, add text/image/shape/link, header/footer/watermark, flatten | Gauntlet S3–S5 |
| **M7** | Fill, calculation/format/validate, Prepare Form, FDF/XFDF | Gauntlet S6 |
| **M8** | Organizer, import/merge/split, retargeting | Gauntlet S7 |
| **M9** | Redaction, sanitize, encryption write, signatures | Gauntlet S8–S9 |
| **M10** | Export/optimize, performance, fuzzer, crash safety | Gates 2 and 7 |
| **M11** | UI states, full icon set, dark theme, HiDPI, responsive | Gate 8 ≥ 90 |
| **M12** | Gauntlet dry-run, fixes, formal run, evidence pack, independent verification | All |

## Immediate queue

1. **The font kinds** (F19–F21): **the standard fourteen are done** for a document that does
   not embed them — twelve of the fourteen now draw from bundled metric-compatible faces, with
   their widths from the built-in tables rather than from the 500-unit default, and the page
   says which face is standing in. `Symbol` and `ZapfDingbats` stay refused: Liberation has no
   equivalent and inventing one would put the wrong glyphs on the page. **Type 3 is next.**
   Type 1 is done and compared against an oracle at 0.982 SSIM. TrueType is done and
   compared against an oracle, and two-byte codes are done with
   it: a composite font's string is split into two-byte CIDs, the pen advances by each CID's
   own entry in the descendant's run-length `/W`, and the mark carries the codes and their
   width so no consumer has to guess (0.99225 SSIM against `mutool`). CFF is done too, and
   differently — a `CFF ` table's glyphs are programs, so they are executed rather than
   walked: a Type 2 stack machine with local and global subroutines, the subroutine-number
   bias tested at both ends of each of its three ranges, every curve and flex operator, and
   the width either present as the first operand or absent (0.98654 SSIM against `mutool`).
   Every glyph of every CFF font on this machine agrees with `ttf-parser`'s independent
   interpreter, coordinate for coordinate — 28 347 outlines, none differing — which is how
   the defect that mattered most was found: a curve operator's points are offsets from the
   point *before* each of them, not all from the pen, and reading them from the pen gives a
   closed, plausible, wrong glyph rather than an error. Encodings are done on top of all of
    that: a code now becomes a glyph *name* through the standard tables and `/Differences`
    before it becomes a glyph number, at 0.98780 SSIM against `mutool` (see "Encodings").
2. **Patterns**: the tiling loop and the colour-space converter, for the painting and
   shading pattern types.
3. **JBIG2 and JPEG 2000 decoders** (F17, F18), or an explicit scope statement if they are
   not going to be built.
4. **Fill the Inspector's right-hand region** from the object model, which is the window work
   that has not been started.
5. **Break the symmetry of the remaining fixtures.** The diagonal-clip page is asymmetric now
   and scored 0.51652 when its clip was a box, where every symmetric fixture had scored above
   0.99 through the same bug. The rest still need the same treatment.

Tier B is now standing — see "The wild corpus found" below — and it has overtaken everything
else on this list, because nothing else in the project can see the problems it can see.

## The wild corpus found

`corpus/wild/` holds 77 real files from pdf.js, PDFBox, NIST, the IRS, the USGS and arXiv,
pinned by SHA-256 in `MANIFEST.toml` and fetched by `cargo xtask corpus fetch`. The harness
is `crates/mangle-render/tests/wild_corpus.rs`; it renders every page at 150 DPI, compares it
with `mutool draw` through `compare()`, compares the text with `pdftotext`, and writes a
report per file into `corpus/wild/report/`. It asserts nothing about any individual file,
because a corpus's job is to find things and a threshold on a document nobody has read yet
turns the first surprise into a permanent red build.

The measurements and their evidence are in `docs/known-diffs.md`. In short: 460 pages
compared, median SSIM 0.6929, 94.8% of pages below 0.95, 35 of 77 files unopenable. Three
findings dominate, in this order:

1. **`render_page` renders a blank page for any Flate-compressed content stream.** All 542
   measured pages drew nothing. The interpreter's own note contains the compressed bytes as
   "operator names", and `Page::decoded_contents` decodes the same stream correctly, so the
   decoder works and the renderer does not call it. Every fixture in the project writes its
   content stream unfiltered, which is why the suite was blind to it. **Done** — see above.
2. **A cross-reference stream carrying a PNG predictor is not decoded**, so the reader falls
   back to scanning, cannot reach objects inside object streams, and reports zero pages. 31 of
   the 34 files that would not open have this shape: both IRS forms, NIST FIPS 197, both USGS
   topo sheets, every Word 2013 export in the corpus, every InDesign output. `qpdf
   --object-streams=disable` on the same content turns 0 pages into 5 and clean into clean.
3. **`render_page` gains a spurious row on any page whose size in points times the scale should
   be a whole number** — `792.0 * (150.0/72.0)` is `1650.0000000000002` in f64 and the `.ceil()`
   turns it into 1651, where `mutool` gives 1650. 80 pages across 17 files became uncomparable,
   including every page of both arXiv papers. **Done** — see above.

(1) is fixed, and it turned out to be the smaller of the two render defects. Re-measured with
it fixed, 103 of 542 pages draw and 439 are still blank — 422 of those because of a font, 17
for other reasons. The dominant one is a `Type1` font whose `/FontFile3` is a bare CFF: a
character code reaches a glyph through the charset, and the charset is not read, so the font
is refused rather than guessed at. **Both font clusters are now done** — the CFF charset is
read (ISO 10581 §5.2, three formats and the Standard Strings list) and the standard fourteen
draw from bundled metric-compatible faces. Until they were, no render number in this project
said anything about fidelity and the arXiv page's SSIM sat at 0.7915 whether or not its
content decoded; it is now 0.88414, and **a full re-run of the corpus is wanted** to
re-baseline the median. It takes two hours, so it is run deliberately rather than in a loop.
6. Tier B with `SOURCES.md`: a real-world corpus, which is the only way to find the encoding
   problems a hand-written fixture cannot express. **Done** — see above.

Settled and no longer queued: images decode and draw, axial and radial shadings paint, the
clip bounds are transformed by exactly one matrix, a clip is now the path the page set rather
than the box around it — the interpreter records the path, its rule and its box together, and
the renderer rasterises the path into a per-pixel mask it multiplies into every fill, so a
diagonal clip is a diagonal and a circular one is round (0.99639 SSIM against `mutool` at
150 DPI) — glyphs fill from embedded TrueType or CFF
outlines through the same filler a path uses, the text matrix and the font size no longer
double-count (a `TJ` kern also displaces the glyph that follows it, not the one at the same
index), and the standard-14 width tables are verified rather than trusted — every one of the
1043 widths was measured against two independent renderers and Adobe's own metrics, so
nobody should re-derive them from a local URW clone. An alias is a different thing from a
number and was not treated as one: the tables are unchanged, and only the names that reach
them grew. A font that declares no `/Widths` now
answers from those tables through its own `/Encoding` rather than from one average advance —
see "Standard-14 substitution".

The clip work is otherwise finished: a clip stays in force until a `W` changes it or a `Q`
restores it, so a mark's rendering depends only on its own record. What is left is the cap —
a page's clip keeps at most 32 paths, past which the outermost are dropped for box-only
culling, and the fix is one shared sweep over the paths rather than a larger number.

One loose end on the write side: `StreamCompression::Flate` is defined but not wired into
`encode_stream`, so a stream chosen for writing is not compressed. What the compressor now
emits is a zlib stream, which `/FlateDecode` names and three other programs can read, so the
work left is the call site rather than the format.
