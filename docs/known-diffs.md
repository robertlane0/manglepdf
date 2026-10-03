# Known differences from the oracles

FINISH.md G3.1: *"per-page best-of-oracles SSIM ≥ 0.95; median ≥ 0.985 across F12–F22, F31,
and Tier B; ≤ 3% of pages below 0.95, each documented in `docs/known-diffs.md` with root
cause and evidence."*

**The Tier-B target is not met, and not narrowly.** Against `mutool draw` at 150 DPI over the
77-file wild corpus: **460 of 542 pages were compared** and the median SSIM is **0.6929**, and
**436 of those 460 pages (94.8%) are below 0.95**. Thirty-four of the 77 files could not be
opened at all; **four cannot** now that D2 is fixed. This page records what the corpus found
and the evidence for each claim.

The corpus is `corpus/wild/`, pinned by SHA-256 in `corpus/wild/MANIFEST.toml`, fetched with
`cargo xtask corpus fetch`, and measured by `crates/mangle-render/tests/wild_corpus.rs`.
That test asserts nothing: the numbers below come from `corpus/wild/report/SUMMARY.md` and
the per-file reports beside it, which are git-ignored and regenerated on every run. Oracle
versions: `mutool 1.28.5`, `pdftotext` (poppler), `qpdf` (system).

> **The SSIM figures below predate [D2](#d2--a-cross-reference-stream-could-never-be-read-at-all-so-every-pdf-15-file-reported-zero-pages)
> and are now too pessimistic by one file in three.** They were measured when 34 of the 77
> files could not be opened at all, and a file that cannot be opened contributes no pages
> and so no SSIM. **73 of 77 now open**, against 43 before, and every one of them reports
> `as written` — the reader uses the file's own cross-reference information and repairs
> nothing. The 460-of-542 comparison count, the 0.6929 median and the 94.8%-below-0.95
> figure all describe a corpus with a third of it missing, and none of them should be read
> as a fidelity number until the corpus is re-run. The two-hour re-run is the next thing
> wanted, and this file will be wrong again until it happens.

**The test is `#[ignore]`d, on purpose.** One run over the corpus takes about two hours, so
leaving it in the default suite makes `cargo test --workspace` unusable. The two cheap tests
beside it — which check the harness itself — still run by default.

Reproduce any line:

```sh
cargo xtask corpus fetch
cargo test -p mangle-render --test wild_corpus -- --ignored --nocapture
```

The 460/542 split matters as much as the SSIM does: **80 pages produced no SSIM at all**,
because their buffers came out one pixel larger than the oracle's and `compare()` refuses
images of different sizes. That was D3, and D3 is now fixed.

### The four files that still do not open

All four are reported, none is guessed at, and each is listed here so the count of 73 is
auditable rather than asserted.

| file | reason | why that is the right answer |
|---|---|---|
| `pdfjs__issue21579.pdf` | `the document is encrypted: incorrect password` | Encrypted, and the password is not derivable. Correct behaviour. |
| `pdfjs__bug1020226.pdf` | `no catalogue` | A 184-byte stub. `qpdf --check` ends `unable to find trailer dictionary while recovering damaged file`. There is nothing to read. |
| `pdfjs__REDHAT-1531897-0.pdf` | `no catalogue` | 871 bytes of fuzzed xref stream. `qpdf --check` ends `unable to find /Root dictionary`, having already rejected object 13 for `overflow/underflow converting 166666666666666666666666666`. |
| `pdfjs__issue19484_1.pdf` | `dangling reference: the page tree root is missing` | **A misdiagnosis, and the one real thing left here.** The file is encrypted — its xref stream carries `/Encrypt 16 0 R`, its manifest lists `encrypted`, and `qpdf` also refuses it with `invalid password`. This reader accepts an empty user password and then cannot read the content: the catalogue resolves, but `/Pages` is object 13 inside object stream 4, and that container decodes to **0 bytes** from 50. So a document that needs a password is reported as one with a broken page tree. The error is in the encrypted-content path, not in the cross-reference reader, and fixing it means deciding whether an empty password that validates should be trusted. Left as it is rather than papered over. |

---

## D1 — `render_page` rendered a blank page for any Flate-compressed content stream

**Severity: was critical — every page of every file. FIXED, and it was smaller than it looked.**

`render_page` called `Page::contents`, which returns the content stream's *encoded* bytes,
and handed them to the content-stream interpreter. The interpreter then reported the
compressed bytes as operator names and drew nothing. The decoder was never at fault:
`Page::decoded_contents` existed, worked, and was used by text extraction and by the CLI.

One line in `crates/mangle-render/src/page.rs`:

```rust
let content = page.contents(doc);          // encoded bytes
let content = page.decoded_contents(doc);  // decoded bytes
```

### Why the fixture suite missed it

Every hand-written fixture wrote its content stream unfiltered. `text_page`'s own comment
says the choice is deliberate — an embedded font program is written uncompressed so that a
comparison measures outlines rather than the font loader — and `page_with` wrote content in
the clear for the same reason. Nothing in the suite ever asked the renderer to inflate
anything, so no test could tell a renderer that decodes content from one that does not.

The fixture is now `page_with_flate`, which is `page_with`'s content behind `/FlateDecode`
and nothing else. Four tests in `crates/mangle-render/tests/pages.rs` hold it down, and all
four fail against the old code:

| test | what it pins |
|---|---|
| `a_compressed_content_stream_is_decoded_and_not_read_as_operators` | the regression: same content, filtered, same three marks and same four pixels |
| `a_content_stream_that_will_not_decode_is_reported` | a damaged `FlateDecode` names itself in the notes |
| `a_content_stream_behind_an_unknown_filter_is_reported_by_name` | an unimplemented filter is named, and its bytes are not drawn |
| `an_array_of_content_streams_names_the_one_that_failed` | a note says *which* of several `/Contents` streams failed |

### What the fix actually bought, measured

Before, **all 542 pages of the 77-file wild corpus rendered with `marks: 0` and no ink.**
After, 103 pages draw and 439 are still blank. So D1 was the cause of the blankness on
19% of pages, and on the other 81% it was hiding a second, larger defect (D7) behind silence.

The page this finding was opened with barely moves, and the reason matters:

| `gov__arxiv-1206.5537.pdf` page 1 | marks | ink pixels | SSIM |
|---|---|---|---|
| before | 0 | 0 | 0.791534 |
| after | 58 | **0** | 0.791534 |

Identical to six places. **The page is blank both before and after**, for two different
reasons: before because the content was never decoded, after because all seven of its fonts
refuse (D7). The SSIM of a blank page against a page with 5.4% ink is 0.7915 whatever the
reason for the blankness is, so this figure could not have moved and its not moving is not
evidence that the fix failed. Two other pages show the difference D1 made:

| page | before | after |
|---|---|---|
| `pdfjs__alphatrans.pdf` 1 | 0 marks, 0 ink, **0.89054** | 12 marks, 373 262 ink, **0.88921** |
| `pdfbox__openoffice-test-document.pdf` 1 | 0 marks, 0 ink, **0.99954** | 4 marks, 315 ink, **0.99998** |

`alphatrans` goes *down* by 0.0013 while gaining 373 262 ink pixels, because the page has a
shading named `/Sh1` that its resources do not define, so the part we now draw is drawn
without it. Drawing a page wrongly scores worse than not drawing it, and it also reports
why — which is the trade the second half of this fix is for.

**The high scorers in the old "What is not wrong" list were an artefact of this bug.**
`openoffice-test-document.pdf` scored 0.9995 while rendering nothing whatsoever, and scores
0.99998 now that it draws its 315 pixels. A blank page compared against a nearly-blank
oracle is a high score for the wrong reason, and the median was computed over 436 pages of
which the great majority were blank.

### The second half: a stream that will not decode has to say so

Decoding the content is the fix; *reporting* it is what stops the next file being another
two-hour mystery. A page that lost its content to a broken filter and a page with no content
look identical on screen and mean opposite things, so the reason now travels with the bytes:

- `Resolver::decoded_full` is the new primitive and returns the decoder's own `Decoded`,
  notes and all; `Resolver::decoded` is defined in terms of it, so the two cannot disagree.
- `Page::decoded_contents_full` concatenates the streams and prefixes every note with which
  stream it came from — `content stream 2 of 4: FlateDecode: input ended before a block
  header` — because `/Contents` is an array at least as often as it is one stream and "the
  second of four failed" is a different finding from "something failed".
- `Decoded` gained an `encoded` flag, because "the decode went wrong" and "these bytes are
  not decoded" are different facts. A truncated `FlateDecode` gives a prefix that is worth
  drawing; an **unimplemented** filter gives back bytes that are still encoded, and handing
  those to the interpreter is D1 all over again. When `encoded` is set the content path
  refuses and reports. That test deliberately writes readable content behind
  `/NoSuchDecode`, so that a renderer which drew it anyway would produce a plausible page
  and pass.

Nothing here guesses. There is no fallback that treats encoded bytes as content, and no
substitute drawn in place of content that could not be read.

---

## D2 — a cross-reference stream could never be read at all, so every PDF 1.5+ file reported zero pages

**Severity: was critical — 34 of the 77 files. FIXED. The corpus now opens 73 of 77.**

> **This entry previously named the wrong cause.** It said the PNG predictor in
> `/DecodeParms` was not decoded, and the corpus correlation below made that look
> airtight: every file with an xref stream and a predictor failed, and the one file with
> an xref stream and no predictor opened. That correlation was a coincidence of who writes
> xref streams — most PDF 1.5+ producers emit Flate with `/Predictor 12` — and the
> predictor decoder was never at fault. It is exercised by `stream::tests::` and works.
> Two defects sat between `startxref` and the predictor, both upstream of it. The
> correlation table is kept below as a record of how a plausible story survived three
> bisection experiments that all exonerated the thing it blamed.

The 34 failures, by the reader's own reason string:

| reason | files |
|---|---|
| `dangling reference: the page tree root is missing` | **29** |
| `no catalogue` | 4 |
| `the document is encrypted: incorrect password` | 1 |

All of the first two rows were this one bug; the encrypted file is legitimately unreadable
without a password.

### The first defect: `next_object` cannot return a stream

A cross-reference stream is an *indirect stream object* — `N G obj << dict >> stream …`.
`Parser::next_object` is a parser for **direct** objects: it sees `<<`, returns the
dictionary, and stops. It never looks for the `stream` keyword, so it can only ever return
an `Object::Dict`, never an `Object::Stream`. `Document::parse_at` knows this and assembles
the stream itself; `xref::read_section` did not, and asked:

```rust
let Some(Object::Stream(stream)) = p.next_object().ok()? else {
    return None;
};
```

which is unsatisfiable. Every xref stream therefore failed to parse, `read_section`
returned `None`, and the chain came back with no entries and no `/Root`.

### The second defect: `/Index` was tested in a way that can never be true

Even once the stream parsed, its entries were numbered from zero regardless of `/Index`:

```rust
let index: Vec<i64> = match stream.dict.get("Index").and_then(Object::as_i64) {
    Some(_) => /* read the array */,
    None => vec![0, size],          // ← always this branch
};
```

`/Index` is an array, and `as_i64` of an array is always `None`, so `get(..).and_then(..)`
is `None` whether or not `/Index` is present. Both cases fell through to `vec![0, size]`.
An incremental update that touches two objects — `/Index [2399 1 2422 1 2448 2 2451 3]` —
had its seven entries attributed to objects 0 through 6, which is how
`pdfbox__acroform.pdf` came to resolve its `/Root 12 0 R` to a **font**.

### Why the symptom was `the page tree root is missing`

With no usable cross-reference information, recovery falls back to scanning for `N G obj`
headers. That scan cannot see objects stored inside object streams, so `/Pages` is
unreachable and the document reports **0 pages** with the note
`dangling reference: the page tree root is missing`. The error names the page tree; the
fault was two layers above it, in the file's table of contents.

### Evidence, `gov__irs-fw4.pdf` — **live IRS Form W-4**, which `qpdf --check` reports as clean

```
$ manglepdf-cli info corpus/wild/gov__irs-fw4.pdf          # before
pages       0
structure   repaired (3 note(s))
            - no cross-reference section at offset 208493
            - 112 objects were located by scanning
            - the catalogue was found by scanning
```

`startxref` is `208493`, and the bytes there are
`3594 0 obj <</Length 44/Type/XRef/Root 3540 0 R/… /Index[3540 1 3563 1 3589 2 3592 3]/W[1 3 1]/DecodeParms<</Columns 5/Predictor 12>>/Filter/FlateDecode>>`.
A well-formed cross-reference stream that qpdf reads without a warning.

Two experiments, run before the cause was found, are what sent the investigation after the
predictor rather than past it. Rewriting the *same content* with the streams expanded and
nothing else changed:

```
$ qpdf --object-streams=disable irs-fw4.pdf irs-fw4-nostream.pdf
$ manglepdf-cli info irs-fw4-nostream.pdf                    # then
pages       5
structure   as written
```

and bisection on `pdfjs__bug1885505.pdf`, which reports `no cross-reference section at
offset 116` over a valid stream: removing `/DecodeParms`, removing `/ID`, removing
`/Index`, changing `/W`, correcting `/Length` by ±3 bytes, and rewriting every bare CR as
LF each left the failure in place. Every one of those edits was upstream of the predictor
and none touched the actual fault, which is that the stream object was never assembled at
all. The fix was found by asking what `next_object` returns for `N G obj <<…>> stream`, not
by reading the file harder.

### The result

```
$ manglepdf-cli info corpus/wild/gov__irs-f1040.pdf
pages       2
structure   as written
objects     2452
```

The reader now uses the file's own cross-reference information: **`as written`, with no
repair at all**, on every file this bug had broken. Page counts match `pdfinfo` on every one
of the 73 files that open.

| | before | after |
|---|---|---|
| files opened | 43 | **73** |
| files not opened | 34 | **4** |
| of the 29, `the page tree root is missing` | 29 | **0** |

Of the 29, 27 write a cross-reference stream and two (`pdfjs__issue19484_1.pdf`,
`pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf`) write a classic table with no `/Root` at all; the
latter now opens by rebuild and is reported as repaired.

Also fixed by this: **IRS Form 1040**, **NIST FIPS 197** (52 pp), **NIST SP 800-88** (44 pp),
NIST IR 7657, both USGS 1:25000 topo sheets, all six Microsoft Word 2013 exports carrying
annotations, both JPEG 2000 files, every Adobe InDesign output in the corpus, and
`bug1885505.pdf` (a pdfTeX paper).

### Two smaller defects the same code was hiding

Both were ways of *inventing* structure, which is the one thing this reader must never do,
and both were found by the fix above rather than looked for.

**A damaged cross-reference stream was accepted as if it were whole.** `read_xref_stream`
returned whatever entries it managed to read when the decoded body ran out early, so a
stream that decoded to a third of what `/Index` promised produced a third of a table and
was believed. `pdfjs__PDFBOX-3148-2-fuzzed.pdf` has exactly this: a corrupt `/ASCII85Decode`
body that qpdf also rejects, promising `/Index [0 18]` and delivering three entries. The
section is now refused, so recovery rebuilds the file by scanning and says so.

**An object number a `u32` cannot hold was renumbered onto object 0.** Both the classic
table reader and the stream reader did `u32::try_from(start.saturating_add(i)).unwrap_or(0)`,
so a corrupt subsection header silently overwrote the free-list head with an unrelated
offset and the rest of the file looked intact. `pdfjs__GHOSTSCRIPT-698804-1-fuzzed.pdf`
carries the header `4294967296`; the table is now refused and the file opens by rebuild,
correctly reported as repaired.

---

## D3 — the page canvas gains a spurious row whenever the size in points times the scale should be a whole number

**Severity: was high. FIXED. 80 pages across 17 files had become impossible to compare.**

`render_page` computed `(points * scale).ceil()` with the division hoisted into `scale`.
In IEEE-754:

```
792.0 * (150.0/72.0) == 1650.0000000000002   ->  ceil == 1651
792.0 * 150.0 / 72.0 == 1650.0               ->  ceil == 1650
561.6 * (150.0/72.0) == 1170.0000000000002   ->  ceil == 1171
960.0 * (150.0/72.0) == 2000.0000000000002   ->  ceil == 2001
```

So US Letter (612 × 792 pt) came out **1651 rows tall where `mutool` produces 1650**, and a
960-point side came out one column too wide. `compare()` refuses to compare images of
different sizes, so every such page was recorded as
`size disagreement: we rendered 1275x1651, mutool rendered 1275x1650` and **scored nothing**.
All 80 had the same shape — one dimension exactly one pixel too big:

| we rendered | mutool rendered | pages |
|---|---|---|
| 1275x1651 | 1275x1650 | 70 |
| 1153x1651 | 1153x1650 | 4 |
| 1250x1751 | 1250x1750 | 2 |
| 915x901 | 915x900 | 1 |
| 2001x1125 | 2000x1125 | 1 |
| 1575x1171 | 1575x1170 | 1 |
| 1020x1531 | 1020x1530 | 1 |

70 of the 80 are a US-Letter page, which includes **all 23 pages of an arXiv paper** and
**all 12 pages of a second one** — the corpus was measuring almost no LaTeX at all.

### What was wrong, and what was not

The ceiling itself is **correct** and was kept. A page of 208.33 pixels needs 209 rows to
hold its last third of a pixel, rounding down clips it, and `mutool` rounds up too. Rounding
to *nearest* would have fixed the Letter page and re-broken the small page, and 208 is a
worse answer than 209: it loses part of the page rather than adding an empty row.

What was wrong was that the ceiling was applied to a value floating point had already put a
hair above the integer. `1650.0000000000002` is one unit in the last place above 1650 and
`ceil` turns that into 1651. The error is invisible at small sizes — one ulp of 208.33 is far
too small to reach the next integer — which is exactly why a 100-point page rounded up
correctly and a Letter page never did. So the epsilon has to be *relative*, not absolute:
one wide enough for 1650 would swallow a quarter of a pixel on a 208-pixel page, and one
narrow enough to spare that page would miss 1650.

`pixels_for` in `crates/mangle-render/src/page.rs` now snaps a value within `1e-9` of an
integer — relative, so about 4.5 million ulps — to that integer before applying the
ceiling. 1e-9 is four thousand times narrower than the smallest difference a real page size
can produce: a page 612.0001 points wide is 1275.0002 pixels, so it still rounds up.

### The evidence, before and after

The oracle's own rule was checked directly rather than assumed, by rendering 15 page sizes
from 1×1 to 1224×1584 with `mutool draw -r 150` and reading the PAM headers. `mutool`
matches ceil-with-epsilon-snap on **all 15**; plain `ceil` disagrees on two of them (792×612
and 1224×1584) and plain `round` disagrees on six.

Then one affected file, `gov__arxiv-1206.5537.pdf` page 1 (MediaBox 612 × 792), rendered
and compared the way the harness does:

| | we rendered | mutool rendered | SSIM |
|---|---|---|---|
| before | 1275x1651 | 1275x1650 | *not measured — size disagreement* |
| after | **1275x1650** | 1275x1650 | **0.7915** (rms 35.4, max delta 224, 116 023 of 2 103 750 pixels above tolerance) |

The off-by-one is gone and the page is compared. The 0.79 is D1 talking — that page's
content stream is Flate-compressed and is drawn blank — so the off-by-one was hiding a real
defect behind an absence of a measurement, which is the worst way for it to hide.

Two tests hold this down in `page.rs`: `a_whole_number_of_pixels_is_not_rounded_up_to_one_more`
(1275x1650, 1650x1275, 2000x1125, 2550x3300) and `a_fractional_page_size_still_rounds_up`
(209x209 for 100pt, 1240x1755 for A4, 600x600), alongside the original
`a_page_buffer_rounds_up_so_the_edge_is_not_clipped`. A fix that traded one for the other
would fail the second.

The existing fixtures did not catch this either: they are 200 × 200 pt and similar, where
`200 × (150/72) = 416.666…`, and both renderers round up to 417.

---

## D4 — an uncompressed cross-reference stream is not read

**Severity: was high. FIXED by [D2](#d2--a-cross-reference-stream-could-never-be-read-at-all-so-every-pdf-15-file-reported-zero-pages) — it was the same bug.**

`pdfjs__bug1539074.pdf`, a pdfTeX-1.40.17 page, whose only cross-reference section is
`<</Type /XRef /Index [0 18] /Size 18 /W [1 2 1] /Length 72>>` — **no `/Filter` at all**, so
the entries are raw bytes.

```
$ manglepdf-cli info bug1539074.pdf                         # before
pages       0
            - no cross-reference section at offset 5715
```

The absence of `/Filter` had nothing to do with it. This file has no `/DecodeParms` either,
which is what made it look like the odd one out that would separate "uncompressed" from
"predictor", and it is the reason D2's correlation looked exact. The section failed for the
same reason as every other: `next_object` returns the stream's dictionary and never the
stream. Now:

```
$ manglepdf-cli info bug1539074.pdf                         # after
pages       1
structure   as written
```

It is left as its own entry because it is the file that falsified the predictor theory, and
that is worth being able to point at.

---

## D5 — the fax scan on a multi-page TIFF page rendered as a black rectangle

**Severity: was high — 0.38414 SSIM, 60.6% of pixels wrong. FIXED as a codec defect. What is
left on this page is a placement defect in the image path, which is not this one.**

`pdfbox__multitiff.pdf` page 1, measured against `mutool draw` at 150 DPI:

| | SSIM | RMS | above tolerance | ink ours / oracle |
|---|---|---|---|---|
| before any of the image work | 0.38414 | 197.78 | 1 318 947 | 59.0% / 8.0% |
| layout and value split apart | 0.91170 | 72.09 | 173 990 | 0.0% / 8.0% |
| **and the T.6 codec reading the coding line's coordinates** | **0.86317** | **88.84** | **264 199** | **7.98% / 8.0%** |

The last row scores lower than the one above it and the page is closer to right, which is
worth being explicit about rather than leaving to be misread: 0.91170 was the score for
drawing nothing at all, and a page with no ink on it agrees very well with a page that is 92%
paper. The renderer now decodes the whole scan and puts 173 595 pixels of ink where the
oracle has 173 990.

What was wrong is recorded in full in `docs/STATUS.md` and `PLAN.md`. There were four defects
in one place, and each was invisible until the one before it was fixed, because a fax decoder
hands back one byte per pixel and *layout* (which byte a pixel lives in) and *value* (what a
sample is worth) are two different facts that only one number was standing in for.

The fifth was the codec underneath. Decoding this page's image — object 12, 344 by 287,
`/K -1` — recovers **287 of 287 rows, byte for byte against `libtiff`, with 0 damaged rows
and nothing truncated**, where it recovered 15 before. The four defects in `decode_line_2d`
were: run lengths added to `b1`, a position on the *reference* line, instead of to `a0`, the
coding line's own current element; `b1_b2` choosing the reference element by the *reference*
line's colour at `a0` rather than the coding line's; the changing elements' polarity inverted,
so every element was labelled with the colour it changes away from; and three of the eight
`MODE_CODES` entries mistranscribed with a ninth — *Vertical(+3)* and the codes `0000010` and
`0000001` among them — missing or wrong. Against the frozen `libtiff` ground truth, **all 15
cases now match byte for byte**, 896 of 896 rows.

What is still wrong on this page is **not** the codec and never was. The page's content is a
full-page colour scan — object 12 is 344 by 287 samples covering the whole 595.28 by 496.64
point width and height the content stream gives it, not a numeral, and it is fully decoded.
It lands in the wrong device rows, which is a placement defect in the image path rather
than a codec one.

---

## D6 — text extraction against `pdftotext`

**Severity: not a defect; a statement of what exists.**

Word-level F1 over the corpus runs from 0.000 to 1.000 with a median near zero. This measures
a **placeholder extractor in the test**, not the product: `mangle-text` is an empty crate, so
`our_words` in `wild_corpus.rs` walks the glyph marks and maps each code through the font's
`/Encoding` and the Adobe glyph list. It is right about simple fonts with a standard or
WinAnsi encoding and wrong about everything else, because a composite font's code is a CID
and wants a `/ToUnicode` CMap. The two files scoring 1.000 are scanned pages with no text at
all, where an empty extractor and an empty oracle agree perfectly.

Every report says so next to the number, so the F1 is never later mistaken for a verdict.
FINISH.md G3.3 is not yet assessable.

---

## What is not wrong

Worth recording, because a corpus that only finds faults is as misleading as one that only
finds faults being called a corpus.

**Read this list with D1 in hand.** Before that fix every one of these pages rendered with
`marks: 0` and no ink at all, so a score near 1.0 meant *our page and the oracle's page were
both nearly empty*. `openoffice-test-document.pdf` has 315 ink pixels on it now and scored
0.9995 while drawing nothing; the others are in the same position to a greater or lesser
degree. What follows is evidence that these files are *not* resisted by the parser — which is
still worth knowing — and it is not evidence that they render correctly.

Where pages *did* draw something:

- `pdfbox__openoffice-test-document.pdf` — **0.9995**
- `pdfbox__PDFA3A.pdf` (Word 2016 → PDF/A-3a, tagged, with transparency) — **0.9962**
- `pdfjs__arial_unicode_ab_cidfont.pdf` — **0.9982**
- `pdfjs__Pages-tree-refs.pdf` — **0.9941**
- `pdfjs__issue10529.pdf` (carries Adobe InDesign private data) — **0.9886**
- `pdfjs__issue20324.pdf` — **0.9720**
- `pdfjs__issue19360.pdf` — **0.9784**
- `pdfjs__cidfont_cmap_overflow.pdf` — **0.9590**
- `pdfjs__bug1795263.pdf` — **0.9564**

`ArabicCIDTrueType.pdf` scores 0.9646 and *does* draw, so composite-font glyph lookup and
the Arabic shaping order are in reasonable shape. And `bug1980958.pdf`, a 219-byte truncated
file, scores 1.0000: both renderers recover the same thing from it, which is what a graceful
degradation looks like.

---

## D8 — the CFF charset was read from the wrong offset, and looked plausible while it did it

**Severity: silent, and the reason the charset reader is written against an oracle rather
than against the specification text.**

D7's first half is fixed: a name-keyed CFF charset is read, in all three of its formats, and
`code → /Encoding name → charset name → GID → CharStrings` reaches a glyph. Two pages
measured blank now draw, against `mutool draw` at 150 DPI:

| page | SSIM before | SSIM after | our ink before | our ink after | `mutool`'s ink |
|---|---|---|---|---|---|
| `gov__arxiv-1206.5537.pdf` p1 (22 bare CMR/CMMI fonts, format 0) | 0.7915 | **0.8784** | 0 | **61 262** | 130 395 |
| `pdfjs__freeculture.pdf` p4 (9 bare CFF fonts, format 1) | 0.9476 | **0.9926** | 0 | **24 954** | 25 929 |

`freeculture` p4 is now within 4% of `mutool`'s ink at 0.9926 SSIM. The arXiv page draws
about half of what `mutool` draws, and the rest of that page is the base-14 stamp down its
margin — D7's second half, still missing.

What is worth recording is not the fix but how it went wrong first. The charset reader was
written from a description of the three formats that said each begins with a `u16` count.
**None of the three does.** Every charset is a format byte followed by entries, and the count
is the glyph count from the CharStrings INDEX:

* format 0 — one SID per glyph after `.notdef`, and no count;
* formats 1 and 2 — runs of `{first: u16, nLeft}`, one after another, and no count.

A reader that assumes a count reads a real entry as a header. For format 0 the first entry's
SID is usually below 256, so it reads as a count of **zero** and the whole charset comes back
empty. That is what happened to every font in the wild corpus: **74 charsets, 0 glyph names
resolved.** The three test builders had been written to match the same mistake, so all three
passed. The cross-check against `fontTools` over the corpus is the only thing that found it.

Two more layout facts came out of the same cross-check, and both are the shape one assumes
wrong:

* **A String INDEX entry has no length byte.** The length is the INDEX's own offset
  arithmetic. Across the 84 String INDEX entries in the corpus, not one begins with a byte
  equal to its own length, which is what a `Pascal string` would. A reader that strips a
  length byte reads the first character as a number, so for a name starting with a capital
  letter it truncates at 65 characters and refuses the rest.
* **The first glyph a run names is glyph 1, not glyph 0**, because `.notdef` is glyph 0 and
  no format lists it.

The 391-name Standard Strings were transcribed and then checked against two independent
sources, which agree on all 391 entries: Ghostscript's `CFFStandardStrings` pseudo-encoding,
whose own source annotates every name with the SID it holds, and the Adobe Glyph List
toolchain's `cffStandardStrings`.

The standing check is `crates/mangle-font/tests/charset_real.rs`: it lifts every name-keyed
CFF out of the corpus, hands the same bytes to `fontTools` and to this reader, and asserts
they agree on **every glyph name**. It reports

```
74 charsets, 1874 glyph names agree with fontTools, 0 do not, 2 unreadable
```

The 2 unreadable are a Private DICT this cannot bound — a different defect, and not the
charset.

---

## D7 — 78% of the corpus is blank because the font cannot be read, not because the content could not

**Severity: critical, and it is what D1 was hiding. 422 of 542 pages.**

> **Partly fixed — see [D8](#d8--the-cff-charset-was-read-from-the-wrong-offset-and-looked-plausible-while-it-did-it).**
> The charset half is done: a name-keyed CFF font's codes reach glyphs and the 355-page
> refusal is gone. The counts below were measured before that and are kept as they were
> measured. The remaining half — 67 pages naming a font that is not embedded at all — is
> untouched.

D1 is fixed and the corpus was re-measured, and this is what the numbers say. All 542 pages,
rendered at 150 DPI with the fix in place:

| | pages |
|---|---|
| draw something | **103** |
| still blank | **439** |

And of the 439 still-blank pages, the notes say why — which is the first time the corpus has
been able to answer that question at all:

| the reason in the notes | pages |
|---|---|
| a bare CFF font whose charset is not read | **355** |
| the font has no `/FontDescriptor` at all | **67** |
| something else | 17 |

22 of the 76 readable files are blank on every page. The other 17 are JPEG 2000 and JBIG2
images with no decoder (9), images claiming 0 by 0 pixels (4), two CID-keyed CFF variants,
one colour space, one truncated `FlateDecode` stream, and one page whose content still does
not parse.

### The dominant one: a bare CFF's charset is not read

355 pages — 65% of the whole corpus, and most of the LaTeX and comic-book material in it —
carry fonts like this:

```
$ qpdf --show-object=18 corpus/wild/gov__arxiv-1206.5537.pdf
<< /BaseFont /JEKHOJ+CMR10 /Encoding 612 0 R /FirstChar 0 /FontDescriptor 19 0 R
   /LastChar 122 /Subtype /Type1 /Type /Font /Widths [...] >>
```

A `Type1` simple font whose `/FontDescriptor` carries a `/FontFile3` — a **bare** CFF table,
with no `sfnt` wrapper and therefore no `cmap`. A character code reaches a glyph like this:

```
code  ->  /Encoding /Differences name  ->  CFF charset: name -> GID  ->  CharStrings
```

The first and last steps work. The middle one did not: `Program::code_refusal` refuses
rather than guess, because the charset is exactly the table that maps a glyph *name* to a
glyph number, and nothing here read it. So the whole font was refused on the first glyph and
every character on the page in it went undrawn. The note it left is accurate and was recorded
on every such page:

> the `/FontFile3` of `/R18` it is a bare CFF font whose codes name glyphs through an
> `/Encoding` this does not resolve, so no character on the page in it can be drawn

Refusing was the right call at the time and the notes it produced are why this was a two-hour
fix rather than a two-week mystery. The charset is now read — see [D8](#d8--the-cff-charset-was-read-from-the-wrong-offset-and-looked-plausible-while-it-did-it) —
and a name the charset lacks is a reason rather than a guess, which is the same rule applied
one level down.

### The second: fonts that are not embedded at all

67 pages name a base-14 font with no `/FontDescriptor` — `Times-Roman` on the arXiv page,
which is the vertical stamp down its left margin. A viewer with the standard 14 font
programs built in draws it; one without does not, and says so. This is a smaller job than
the charset and worth ordering first, because it is also what the `manglepdf-cli extract`
path needs to agree with `pdftotext`.

### What this means for the numbers in this file

The Tier-B median of 0.6929 was measured over pages that were blank, and blank pages scored
against pages with ink. It is a real measurement of the defect it describes, and the defect
it describes is overwhelmingly not the one named at the top of it. Nothing in the SSIM
figures in this file should be read as a fidelity claim until D7 is fixed, because a corpus
that cannot draw a single glyph cannot say anything about fidelity.